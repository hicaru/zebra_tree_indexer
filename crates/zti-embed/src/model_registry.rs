use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::pooling::PoolingStrategy;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub model_id: String,
    pub onnx_path: PathBuf,
    pub tokenizer_path: PathBuf,
    pub dim: usize,
    pub max_length: usize,
    pub pooling: PoolingStrategyEnum,
    pub query_prefix: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PoolingStrategyEnum {
    Mean,
    Cls,
}

impl From<PoolingStrategyEnum> for PoolingStrategy {
    fn from(v: PoolingStrategyEnum) -> Self {
        match v {
            PoolingStrategyEnum::Mean => PoolingStrategy::Mean,
            PoolingStrategyEnum::Cls => PoolingStrategy::Cls,
        }
    }
}

pub struct ResolvedModel {
    pub onnx_path: PathBuf,
    pub tokenizer_path: PathBuf,
}

struct FamilyQuirks {
    query_prefix: Option<&'static str>,
    pooling: PoolingStrategyEnum,
}

const BERT_QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

const BERT_FAMILY_PREFIXES: &[&str] = &["bge-", "mxbai-", "gte-", "e5-"];

const ONNX_CANDIDATES: &[&str] = &[
    "onnx/model.onnx",
    "model.onnx",
    "onnx/model_quantized.onnx",
];

const TOKENIZER_CANDIDATES: &[&str] = &["tokenizer.json", "onnx/tokenizer.json"];

fn lookup_quirks(model_name: &str) -> Option<FamilyQuirks> {
    let lower = model_name.to_lowercase();

    if lower.contains("bge-small-en-v1.5")
        || lower.contains("bge-base-en-v1.5")
        || lower.contains("bge-large-en-v1.5")
    {
        return Some(FamilyQuirks {
            query_prefix: Some(BERT_QUERY_PREFIX),
            pooling: PoolingStrategyEnum::Cls,
        });
    }
    if lower.contains("mxbai-embed-large") {
        return Some(FamilyQuirks {
            query_prefix: Some(BERT_QUERY_PREFIX),
            pooling: PoolingStrategyEnum::Cls,
        });
    }
    if BERT_FAMILY_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return Some(FamilyQuirks {
            query_prefix: Some(BERT_QUERY_PREFIX),
            pooling: PoolingStrategyEnum::Cls,
        });
    }
    None
}

pub fn resolve_profile(model_id: &str) -> Result<ModelProfile> {
    let files = resolve_model_files(model_id)?;
    let quirks = lookup_quirks(model_id);

    let pooling = quirks
        .as_ref()
        .map(|q| q.pooling)
        .unwrap_or(PoolingStrategyEnum::Mean);
    let query_prefix = quirks.and_then(|q| q.query_prefix.map(String::from));

    // dim and max_length are placeholders; engine overrides them after the
    // ONNX session and tokenizer are loaded.
    Ok(ModelProfile {
        model_id: model_id.to_string(),
        onnx_path: files.onnx_path,
        tokenizer_path: files.tokenizer_path,
        dim: 0,
        max_length: 512,
        pooling,
        query_prefix,
    })
}

pub fn resolve_model_files(model_id: &str) -> Result<ResolvedModel> {
    let p = Path::new(model_id);
    if p.exists() {
        resolve_local(p)
    } else {
        resolve_hf(model_id)
    }
}

fn resolve_local(p: &Path) -> Result<ResolvedModel> {
    let (dir, explicit_onnx) = if p.is_dir() {
        (p.to_path_buf(), None)
    } else if p.extension().and_then(|s| s.to_str()) == Some("onnx") {
        let parent = p
            .parent()
            .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", p.display()))?;
        (parent.to_path_buf(), Some(p.to_path_buf()))
    } else {
        anyhow::bail!(
            "{} is neither a directory nor a .onnx file",
            p.display()
        );
    };

    let onnx_path = match explicit_onnx {
        Some(file) => file,
        None => find_onnx_in(&dir)?,
    };
    let tokenizer_path = find_tokenizer_in(&dir)?;

    tracing::info!(
        onnx = %onnx_path.display(),
        tokenizer = %tokenizer_path.display(),
        "using local model files"
    );

    Ok(ResolvedModel {
        onnx_path,
        tokenizer_path,
    })
}

fn find_onnx_in(dir: &Path) -> Result<PathBuf> {
    for c in ONNX_CANDIDATES {
        let p = dir.join(c);
        if p.exists() {
            return Ok(p);
        }
    }

    let mut found: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("onnx") {
            if found.is_some() {
                anyhow::bail!(
                    "multiple .onnx files in {} — pass the specific file path \
                     instead of the directory",
                    dir.display()
                );
            }
            found = Some(path);
        }
    }
    found.ok_or_else(|| anyhow::anyhow!("no .onnx file found in {}", dir.display()))
}

fn find_tokenizer_in(dir: &Path) -> Result<PathBuf> {
    for c in TOKENIZER_CANDIDATES {
        let p = dir.join(c);
        if p.exists() {
            return Ok(p);
        }
    }
    anyhow::bail!(
        "no tokenizer.json found in {} (download it from the model's HF repo, \
         e.g. https://huggingface.co/<owner>/<name>/resolve/main/tokenizer.json)",
        dir.display()
    )
}

fn resolve_hf(model_id: &str) -> Result<ResolvedModel> {
    let model_dir = zti_common::paths::models_dir()?.join(model_id.replace('/', "_"));
    std::fs::create_dir_all(&model_dir)?;

    let onnx_path = model_dir.join("model.onnx");
    let tokenizer_path = model_dir.join("tokenizer.json");

    let parts: Vec<&str> = model_id.splitn(2, '/').collect();
    let (owner, name) = match parts.as_slice() {
        [o, n] => (*o, *n),
        _ => anyhow::bail!("invalid model_id: expected 'owner/name', got '{}'", model_id),
    };

    if !onnx_path.exists() {
        tracing::info!("downloading ONNX model for {}", model_id);
        let client = hf_hub::HFClientSync::new()?;
        let repo = client.model(owner, name);
        let downloaded = try_download(&repo, ONNX_CANDIDATES)?;
        std::fs::copy(&downloaded, &onnx_path)?;
    }

    if !tokenizer_path.exists() {
        tracing::info!("downloading tokenizer for {}", model_id);
        let client = hf_hub::HFClientSync::new()?;
        let repo = client.model(owner, name);
        let downloaded = try_download(&repo, TOKENIZER_CANDIDATES)?;
        std::fs::copy(&downloaded, &tokenizer_path)?;
    }

    Ok(ResolvedModel {
        onnx_path,
        tokenizer_path,
    })
}

fn try_download(
    repo: &hf_hub::HFRepositorySync<hf_hub::RepoTypeModel>,
    candidates: &[&str],
) -> Result<PathBuf> {
    let mut last_err: Option<String> = None;
    for fname in candidates {
        match repo
            .download_file()
            .filename(fname.to_string())
            .send()
        {
            Ok(p) => {
                tracing::debug!(filename = fname, "downloaded {}", p.display());
                return Ok(p);
            }
            Err(e) => {
                tracing::debug!(filename = fname, "candidate not found: {}", e);
                last_err = Some(format!("{} -> {}", fname, e));
            }
        }
    }
    anyhow::bail!(
        "none of {:?} could be downloaded ({})",
        candidates,
        last_err.unwrap_or_else(|| "no attempts".to_string())
    )
}
