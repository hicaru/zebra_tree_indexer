# Context: READ-ONLY performance analysis for `zebra_tree_indexer`

User request: read-only, in-depth analysis of allocations, blocking, async misuse, unnecessary copies in `/Users/hicaru/projects/zebra/zebra_tree_indexer`; correlate `/Users/hicaru/projects/zebra/daemon_profile_v2.txt` macOS `sample` stacks with Rust code; return structured findings, improvement plan ideas, tests/benchmarks. No code changes should be made.

## Profile evidence (macOS sample)

Key stacks from `/Users/hicaru/projects/zebra/daemon_profile_v2.txt`:

- Main thread mostly idle in `Runtime::block_on`; shutdown drops have Metal/Lance/tokenizer deallocation noise:
  - lines 27-33: `zti_daemon::run_daemon` -> `Runtime::block_on` -> parked.
  - lines 37-141: drop paths include `candle_core::metal_backend::device::MetalDevice`, `lance_core::cache::LanceCache`, `zti_embed::tokenizer::Tokenizer`.
- Hot indexing embed stack:
  - line 180: `zti_pipeline::indexer::index_project` sampled 3150.
  - line 181: `tokio::runtime::scheduler::multi_thread::worker::block_in_place`.
  - line 183: `zti_embed::engine::EmbedEngine::embed_batch_tokenized_sync`.
  - lines 184-189: `Tensor::to_device` -> `MetalStorage::to_cpu` -> `MetalDevice::wait_until_completed` -> `Commands::flush_and_wait` -> `-[MTLCommandBuffer waitUntilCompleted]` (3135 samples in that branch).
  - lines 193-197: `_platform_memmove` under Metal/`to_vec1` copies.
- LanceDB write/upsert stack:
  - line 221: `zti_store::upsert::upsert_batch` under `index_project`.
  - line 708: same path with 25 samples.
  - total section line 25771 onward: `merge_insert` and `upsert_batch` both appear with 58 recursive stack hits.
- File walk / synchronous file I/O:
  - line 535: `zti_pipeline::manifest::walk_source_files`.
  - lines 536, 540: `std::fs::read_to_string`.
  - total section line 25771 onward: `open`/`__open`/Rust `File::open_c` appear ~97-99 hits; `pread` 57 hits.
- Tokenizer/Rayon allocation hot path:
  - line 1594: Rayon worker thread starts.
  - lines 1600-1644: `rayon_core::registry::ThreadBuilder::run`, nested `rayon::iter::plumbing::bridge_producer_consumer`.
  - lines 1624-1629: Rayon worker in `zti_pipeline::indexer::adaptive_split` -> `zti_embed::tokenizer::Tokenizer::count_tokens` -> `tokenizers::TokenizerImpl::encode`.
  - total section line 25771 onward: `rayon::...bridge_producer_consumer` 352/163/94, `TokenizerImpl::encode` 216, `Vec::spec_extend` 162, malloc/free ~102/101.
- Many mostly parked / oversubscribed threads:
  - line 152: tokio worker.
  - lines 1594, 2193, 2762, ...: Rayon worker threads.
  - lines 6180-6352: multiple `lance-cpu` threads.
  - many later `tokio-rt-worker` threads are blocked in `tokio::runtime::blocking::pool::Inner::run` / condvar wait.
  - line 25642: Metal command queue dispatch.
- Search hot-path maintenance:
  - lines 1079, 1461: `zti_pipeline::search::search`.
  - lines 1123, 1462: `zti_store::chunks_table::ChunksTable::optimize` during search.

Interpretation: the largest sampled indexing stall is not Rust CPU compute but synchronous Metal completion forced by pulling the entire hidden-state tensor back to CPU. Secondary costs are tokenizer/Rayon allocations, LanceDB per-batch merge-inserts, and broad thread oversubscription/parking from Tokio + Rayon + Lance + Metal.

## High-value code evidence

### Daemon/runtime and indexing task

- `crates/zti-daemon/src/lib.rs:75-78`: model loaded synchronously before runtime; `Runtime::new()` uses default multi-thread runtime.
- `crates/zti-daemon/src/handlers/index.rs:50-55`: indexing is launched with `tokio::spawn(async move { index_project(...).await })`, so `index_project` synchronous phases run on a Tokio worker until their first `.await` or explicit `block_in_place`.
- `crates/zti-embed/src/engine.rs:257-264`, `278-286`, `300-308`, `311-312`: async embedding wrappers call synchronous tokenizer first, then `tokio::task::block_in_place` around `embed_batch_tokenized_sync`.
- `crates/zti-pipeline/src/indexer.rs:489-490`: every embedding batch awaits `engine.embed_batch_tokenized_async`, which is `block_in_place`.

Risk/pattern: `block_in_place` is being used for long GPU/CPU work inside a large async pipeline. It prevents reactor starvation for that worker, but under repeated batches plus Lance/Rayon threads it encourages many parked/replacement Tokio workers and complicates cancellation/backpressure. Consider running the entire indexing worker as a bounded `spawn_blocking`/dedicated worker pipeline or separating sync CPU/GPU phases from async DB I/O with bounded channels.

### Metal synchronization and copies

- `crates/zti-embed/src/engine.rs:400-408`: `run_and_pool` builds Metal tensors, calls BERT, then immediately calls `output.to_device(Cpu)?.to_dtype(F32)?.flatten_all()?.to_vec1::<f32>()?`.
- `crates/zti-embed/src/pooling.rs:1-27`: pooling is CPU-only over a full `(seq * dim)` row slice; mean pooling visits all valid tokens, CLS copies first row.
- `crates/zti-embed/src/normalize.rs:1-8`: L2 normalization CPU-only.

Profile correlation: lines 184-189 show the `to_device(Cpu)` path waits on Metal (`waitUntilCompleted`), and lines 193-197 show `memmove` from the transfer/vectorization.

Likely improvement: pool and normalize on device, then transfer only `(batch * dim)` instead of `(batch * seq * dim)`. For CLS, slice/gather first token on device. For mean, masked sum/divide on device. This should remove or shrink `waitUntilCompleted`/`memmove` cost and cut transfer volume by ~`seq` factor. Validate Candle Metal supports needed reductions/slices for current version.

### Tokenizer/Rayon and allocations

- `crates/zti-pipeline/src/indexer.rs:290-335`: chunk generation runs `need_reindex.par_iter()`, and `adaptive_split` may call tokenizer token counting inside Rayon.
- `crates/zti-pipeline/src/indexer.rs:412-435`: all pending chunks are prefixed into a `Vec<Cow<str>>`, then references are collected into a second `Vec<&str>`, then all tokenized encodings are collected before embedding.
- `crates/zti-embed/src/tokenizer.rs:25-38`: `encode_batch` calls `self.inner.encode_batch(texts.to_vec(), false)` (copies refs) and for each encoding clones `ids` and `attention_mask` with `.to_vec()`.
- `crates/zti-embed/src/tokenizer.rs:47-52`: `count_tokens` calls full tokenizer `encode` and drops the encoding.

Profile correlation: total section shows tokenizer encode (216), Rayon bridge (352), Vec extension (162), malloc/free ~100; lines 1624-1629 tie `adaptive_split` directly to tokenizer encode.

Likely improvements:
- Avoid storing a full attention-mask Vec per chunk if masks are all ones for unpadded encodings; keep only `ids: Vec<u32>` and derive mask/valid lengths in `fill_scratch` (`engine.rs:368-375`). Verify tokenizers behavior for special tokens/padding first.
- Consider disabling/narrowing tokenizers internal Rayon in contexts already parallelized by `indexer.rs:290` to avoid nested Rayon. Alternatively make chunk generation sequential or use one bounded Rayon pool.
- Avoid all-at-once `prefixed` and `refs` vectors if possible; stream tokenization/embedding in buckets. If sorting by length is required, collect only compact `(idx, token_len)` metadata or batch by file.

### Chunk/content copies and memory peak

- `crates/zti-pipeline/src/indexer.rs:111-121`: recursive sub-chunks clone `file`, `rel_file`, `qualified`, and allocate `body` via `to_string()` for each subchunk.
- `crates/zti-pipeline/src/indexer.rs:319-325`: text files call `snap.contents.clone()` into `chunk_text_file`; for large text/doc files this duplicates the already retained snapshot content.
- `crates/zti-dsl/src/chunking.rs:173-190`: `chunk_text_file` takes ownership of a `String`; this forced clone is only because `FileSnapshot` owns the content and the chunk API for text is `'static`.
- `crates/zti-pipeline/src/indexer.rs:555-559`: per chunk allocates `appendix_ids: Vec<u32>` from `appendix_for`; `appendix_for` also creates a `HashSet`, `VecDeque`, and output `Vec` for every chunk (`indexer.rs:365-392` in the full file).

Likely improvements:
- Borrow text contents in chunks where possible; only materialize String when building Arrow content.
- Precompute appendix ids per symbol once or reuse per-batch scratch buffers; use `FxHashSet`/smallvec/arrayvec if adding deps is acceptable.
- Consider storing `Arc<str>`/interned file path and qualified strings in intermediate `Chunk` if all-pending must remain all-at-once.

### LanceDB upsert/optimize overhead

- `crates/zti-pipeline/src/indexer.rs:594-620`: every embedding batch builds a `RecordBatch` and immediately `chunks_table.upsert(record).await`.
- `crates/zti-store/src/upsert.rs:9-17`: `upsert_batch` boxes a single-batch reader and executes `table.merge_insert` for that batch.
- `crates/zti-pipeline/src/indexer.rs:713-752`: `upsert_files` builds and upserts one-row `RecordBatch` per changed path.
- `crates/zti-pipeline/src/indexer.rs:669-670`: indexer calls `chunks_table.optimize().await?` and then `build_index` after indexing.
- `crates/zti-pipeline/src/search.rs:163-164`: search calls `chunks_table.optimize().await?` on every non-exhaustive search.
- `crates/zti-store/src/chunks_table.rs:516-520`: `optimize` is `OptimizeAction::All`.

Profile correlation: `zti_store::upsert::upsert_batch` at profile lines 221 and 708; total section line 25771 onward has `merge_insert`/`upsert_batch` 58. `ChunksTable::optimize` appears under search at profile lines 1123 and 1462.

Likely improvements:
- Since modified/removed chunks are deleted before reindex (`indexer.rs` earlier around `delete_for_files`) and chunk IDs are deterministic, chunk writes can likely be append-only for newly generated rows rather than per-batch `merge_insert`; if duplicate safety is needed, perform fewer/larger merge-inserts.
- Coalesce file metadata updates into one `RecordBatch` / one merge, not one row at a time.
- Remove `optimize` from search hot path; make it explicit maintenance or throttle after writes. Re-open/reuse `ChunksTable` handle if possible to avoid table open/schema checks.

### Manifest/file walk memory and no-op index cost

- `crates/zti-pipeline/src/manifest.rs:140-220`: `walk_source_files` walks all files, reads every UTF-8 file into `String`, computes hash, stores `FileSnapshot { contents, ... }` for every current file.
- `crates/zti-pipeline/src/manifest.rs:242-252`: only after all content is read/hashes computed are paths categorized added/modified/unchanged.
- `crates/zti-pipeline/src/indexer.rs` early-return for `need_reindex.is_empty()` happens after `walk_source_files` and `files_table.list`, so no-op runs still read/hash everything.

Profile correlation: `walk_source_files` and `read_to_string` appear at lines 535-540; total section has filesystem open/pread.

Likely improvements:
- First collect path + metadata; compare `(mtime,size)` against previous rows; only read/hash content for new/suspect files. If hash robustness is required, lazily hash only candidates whose metadata changed.
- For full DSL parse after detected changes, read all code files only when necessary; for true no-op avoid content reads entirely.
- Store prior mtime/size already exists in `FileRow`; use it.

## Prioritized improvement plan ideas

1. **Remove per-search optimize** (`search.rs:163-164`) and replace with post-index or periodic maintenance. Low risk, likely immediate latency win.
2. **Coalesce/append writes**: switch chunks to append after pre-delete or batch merge-inserts much larger; batch file metadata (`indexer.rs:620`, `713-752`, `upsert.rs:9-17`). Medium risk; validate duplicate handling and Lance schema/index semantics.
3. **Reduce Metal transfer volume**: pool/normalize on device, transfer pooled embeddings only (`engine.rs:400-408`). Higher complexity, likely biggest indexing win.
4. **Bound sync/parallel work**: make indexing a bounded sync worker or dedicated runtime task; avoid nested global Rayon/tokenizers parallelism; consider runtime builder with explicit `max_blocking_threads` and named threads. Validate progress streaming still works.
5. **Cut tokenization allocations**: remove per-token mask Vec if safe, avoid `texts.to_vec`, avoid all-at-once prefixed/ref vectors if possible.
6. **Avoid no-op full reads**: metadata-first manifest walker; delay full content reads until needed.
7. **Intermediate memory cleanup**: borrow text chunks, precompute appendix ids, reduce cloned path/qualified strings.

## Suggested validation / benchmarks

Functional tests:
- Tokenizer refactor: unit tests that `fill_scratch` masks and valid counts match current outputs for representative BERT tokenizers; include truncation/max_length behavior.
- Write-path change: integration test indexing a temp project, reindex unchanged, modify one file, delete one file; assert no duplicate chunks and correct files/project rows.
- Search optimize removal: search after fresh index and after incremental update still returns expected results; ANN index creation still happens when requested.
- Metadata-first walker: tests for unchanged, modified same size, mtime changes, invalid UTF-8 ignored, generated/skip filters preserved.

Benchmarks/profiling:
- End-to-end: build release `cargo build --release -p zebraindex`; run daemon/index on this repo; collect `sample` before/after and compare stacks: `waitUntilCompleted`, `merge_insert`, tokenizer/Rayon, thread count.
- Add Criterion benches under `benches/` or crate benches for: tokenizer `encode_batch` over representative chunks; recursive chunking on large generated Rust/Markdown; Arrow record-batch construction for N rows; append vs merge-insert on temp LanceDB table.
- Memory: run with macOS Instruments Allocations or `MallocStackLogging=1` + `leaks`/`malloc_history`; track peak RSS from `ps` or `time -l` for indexing.
- Threading: run with `RAYON_NUM_THREADS` varied (1, physical cores) and/or tokenizers parallelism env if available; compare wall time and `sample` parked threads.

## Constraints / risks

- READ-ONLY task: no code edits should be made for this request.
- Candle Metal capabilities must be verified before promising on-device mean pooling/normalization; fallback can be CLS-only optimization or reduce transfer with CPU pooling when unsupported.
- LanceDB API semantics for append vs merge_insert and optimize/index lifecycle need validation against version `0.30` in workspace deps.
- Tokenizers `Encoding` fields are private in 0.23.1; local source shows only `get_ids()`/`get_attention_mask()` accessors, no obvious `into_ids`. Eliminating copies may require wrapping less data (drop mask) rather than moving ids out.
