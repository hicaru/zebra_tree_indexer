use std::sync::Arc;

use anyhow::{Result, bail};
use futures::StreamExt;

use crate::client::RemoteEmbedClient;
use crate::models::RemoteModelInfo;
use crate::provider::RemoteProvider;

/// Approximate token ceiling per HTTP request to a remote provider.
/// Tuned to stay well under typical provider limits.
const DEFAULT_BATCH_TOKENS: usize = 100_000;
/// Bytes-per-token estimate for batch sizing math.
const BYTES_PER_TOKEN: usize = 4;
const DEFAULT_MAX_LENGTH: usize = 4096;
const REMOTE_EMBED_PIPELINE: usize = 4;

async fn probe_dim(client: &RemoteEmbedClient, model_id: &str) -> Result<usize> {
    let rows = client.embed_batch(model_id, &["a"]).await?;
    match rows.into_iter().next() {
        Some(v) if !v.is_empty() => Ok(v.len()),
        _ => bail!("remote probe returned an empty embedding vector"),
    }
}

pub struct RemoteEmbedEngine {
    client: RemoteEmbedClient,
    provider: RemoteProvider,
    model_id: Arc<str>,
    dim: usize,
    /// Effective token ceiling for chunking decisions (from model metadata or default).
    max_length: usize,
    /// Maximum number of characters per single HTTP request (byte-proxy for token budget).
    batch_char_limit: usize,
}

impl RemoteEmbedEngine {
    /// Construct and optionally skip dim-probe when `cached_dim` is known.
    pub async fn connect(
        provider: RemoteProvider,
        api_key: Arc<str>,
        model: &RemoteModelInfo,
        cached_dim: Option<usize>,
    ) -> Result<Self> {
        let client = RemoteEmbedClient::new(provider, api_key)?;
        let dim = match cached_dim {
            Some(d) if d > 0 => d,
            _ => probe_dim(&client, &model.id).await?,
        };
        let max_length = usize::try_from(model.context_length)
            .ok()
            .filter(|len| *len > 0)
            .unwrap_or(DEFAULT_MAX_LENGTH);
        let batch_char_limit = DEFAULT_BATCH_TOKENS.saturating_mul(BYTES_PER_TOKEN);
        Ok(Self {
            client,
            provider,
            model_id: Arc::from(model.id.as_str()),
            dim,
            max_length,
            batch_char_limit,
        })
    }

    #[inline]
    pub const fn provider(&self) -> RemoteProvider {
        self.provider
    }

    #[inline]
    pub const fn dim(&self) -> usize {
        self.dim
    }

    #[inline]
    pub const fn max_length(&self) -> usize {
        self.max_length
    }

    #[inline]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Dynamic batch sizing: split `texts` into provider-sized sub-batches and
    /// pipeline several HTTP requests while preserving response order.
    pub async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let max_items = self.provider.max_batch_items().max(1);
        let mut batches: Vec<Vec<String>> = Vec::with_capacity(texts.len().div_ceil(max_items));
        let mut batch: Vec<String> = Vec::with_capacity(texts.len().min(max_items));
        let mut batch_chars: usize = 0;

        for text in texts {
            let len = text.len();
            if !batch.is_empty()
                && (batch.len() >= max_items
                    || batch_chars.saturating_add(len) > self.batch_char_limit)
            {
                batches.push(std::mem::take(&mut batch));
                batch = Vec::with_capacity(max_items);
                batch_chars = 0;
            }
            batch.push((*text).to_string());
            batch_chars = batch_chars.saturating_add(len);
        }
        if !batch.is_empty() {
            batches.push(batch);
        }

        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        let client = self.client.clone();
        let model_id = Arc::clone(&self.model_id);
        let mut stream = futures::stream::iter(batches.into_iter().map(move |batch| {
            let client = client.clone();
            let model_id = Arc::clone(&model_id);
            async move {
                let refs: Vec<&str> = batch.iter().map(String::as_str).collect();
                client.embed_batch(&model_id, &refs).await
            }
        }))
        .buffered(REMOTE_EMBED_PIPELINE);

        while let Some(rows) = stream.next().await {
            out.extend(rows?);
        }
        Ok(out)
    }

    pub async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let rows = self.client.embed_batch(&self.model_id, &[text]).await?;
        rows.into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("remote embed returned no vector"))
    }

    pub async fn embed_passage(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_query(text).await
    }
}
