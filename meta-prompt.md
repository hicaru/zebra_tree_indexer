# Meta-prompt for next agent

## Goal
Produce or use a performance plan for `zebra_tree_indexer` based on the read-only analysis of `/Users/hicaru/projects/zebra/daemon_profile_v2.txt`, focusing on allocations, blocking, async misuse, unnecessary copies, and Rust source correlations. If implementation is requested later, prioritize low-risk hot-path fixes first.

## Context / evidence
Read `context.md` first. Key facts:
- Profile hot stack: `index_project` -> Tokio `block_in_place` -> `EmbedEngine::embed_batch_tokenized_sync` -> Candle Metal `Tensor::to_device(Cpu)` -> `waitUntilCompleted`; see profile lines 180-189 and code `crates/zti-embed/src/engine.rs:400-408`.
- CPU pooling requires full hidden-state transfer: `pooling.rs:1-27`, `normalize.rs:1-8`.
- Indexing is spawned as normal async task (`zti-daemon/src/handlers/index.rs:50-55`) while large synchronous phases run on Tokio workers; async embed wrappers use `block_in_place` (`engine.rs:257-312`).
- Tokenizer/Rayon allocations: profile total section line 25771 onward shows tokenizer/Rayon/Vec/malloc; code `zti-embed/src/tokenizer.rs:25-38` clones ids and masks and `indexer.rs:412-435` collects all prefixed refs and tokenizations.
- LanceDB overhead: per embed batch calls `chunks_table.upsert(record).await` (`indexer.rs:594-620`), implemented with `merge_insert` (`zti-store/src/upsert.rs:9-17`); files metadata upserts one row at a time (`indexer.rs:713-752`).
- Search calls `chunks_table.optimize().await?` on every query (`zti-pipeline/src/search.rs:163-164`); `optimize` is `OptimizeAction::All` (`zti-store/src/chunks_table.rs:516-520`).
- File walker reads/hashes/stores all source contents before detecting changes (`zti-pipeline/src/manifest.rs:140-220`, `242-252`).

## Success criteria
- Preserve read-only constraint unless explicitly asked to implement.
- Findings/plans cite exact file:line evidence and profile line evidence.
- Plan distinguishes low-risk wins (remove search optimize, batch writes) from high-complexity changes (on-device pooling, indexing pipeline threading).
- Validation includes both correctness tests and end-to-end profiling/benchmark checks.

## Hard constraints
- For the current user request, do not edit project code. Any generated report/handoff files are allowed only as analysis artifacts.
- Do not claim Candle/LanceDB/tokenizers API behavior without verifying against local source or a compile/test.
- Preserve daemon progress streaming and cancellation semantics in any later implementation plan.

## Suggested approach if implementing later
1. Remove/throttle `chunks_table.optimize()` from search hot path; keep post-index maintenance.
2. Coalesce file metadata writes and chunk writes; prefer append-after-delete for chunks or larger merge batches after verifying LanceDB behavior.
3. Reduce tokenization memory: drop stored attention-mask Vec if masks can be derived; reduce all-at-once vectors.
4. Move pooling/normalization to device or otherwise transfer only pooled embeddings from Metal.
5. Restructure indexing so long sync CPU/GPU work is bounded and not spread across Tokio workers via repeated `block_in_place`.
6. Make manifest walker metadata-first for no-op/mostly-unchanged runs.

## Validation
- Run `cargo test --workspace` for correctness.
- Add targeted tests for metadata-first change detection, write dedup/update behavior, and tokenizer mask equivalence.
- Build release and profile indexing this repo using macOS `sample`; compare `waitUntilCompleted`, `merge_insert`, tokenizer/Rayon, thread count, and wall time.
- Use Criterion benches for tokenizer, recursive chunking, Arrow batch construction, and Lance append vs merge.

## Stop / escalation rules
- Stop when the analysis/report includes source-backed findings, risks, and validations; do not continue into edits.
- Escalate/ask for a decision before high-risk design choices: device-side pooling API, append-vs-merge semantics, changing runtime/threading model, or adding new dependencies.

## Resolved questions / assumptions
- Assumed the profile represents daemon indexing with Metal backend and LanceDB writes active.
- Assumed user wanted no source edits; this handoff is analysis-only.
- Tokenizers 0.23.1 local source exposes `get_ids`/`get_attention_mask`; no obvious move-out API was verified, so copy elimination should focus first on not storing masks and reducing intermediate vectors.
