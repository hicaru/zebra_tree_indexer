use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use clap::Subcommand;

use zti_dsl::render::dsl::{DslRenderer, render_files_only};
use zti_dsl::render::tree::AsciiTreeRenderer;
use zti_dsl::DslChunker;
use zti_tree_sitter::{frontend_for, parse_kinds, parse_language};
use zti_ts_core::walker::LanguageFrontend;

#[derive(Subcommand)]
pub enum DslCommands {
    #[command(about = "Show the file tree with numeric IDs")]
    FileTree {
        #[arg(short, long, help = "Glob pattern to filter files")]
        path_glob: Option<String>,
    },
    #[command(about = "Show the DSL symbol map, sectioned by language")]
    ProjectMap {
        #[arg(
            short,
            long,
            help = "Restrict to one language (rs|ts|tsx|dart|sol). Omit to include all."
        )]
        language: Option<String>,
        #[arg(short, long, help = "Glob pattern to filter files")]
        path_glob: Option<String>,
        #[arg(
            short,
            long,
            help = "Filter by kinds: fn, method, struct, enum, class, const, module"
        )]
        kinds: Option<Vec<String>>,
        #[arg(short, long, default_value = "8000", help = "Max tokens")]
        max_tokens: usize,
    },
    #[command(about = "Trace dependency chains for a symbol")]
    DepTree {
        #[arg(short, long, help = "Symbol ID")]
        id: u32,
        #[arg(
            short = 'D',
            long,
            default_value = "callers",
            help = "Direction: callers or callees"
        )]
        direction: String,
        #[arg(short, long, default_value = "3", help = "Max depth")]
        depth: usize,
    },
    #[command(about = "Show the source code of a symbol")]
    SymbolBody {
        #[arg(short, long, help = "Symbol ID")]
        id: u32,
    },
    #[command(about = "Show the source code of multiple symbols")]
    SymbolBodies {
        #[arg(short, long, num_args(1..), help = "Symbol IDs")]
        ids: Vec<u32>,
    },
    #[command(about = "Sequential chunk trace to diagnose chunk-generation hangs")]
    ChunkTrace,
}

pub fn run_dsl(root: &Path, command: DslCommands) -> Result<()> {
    let canonical = root.canonicalize()?;
    let root_cow = canonical.to_string_lossy();

    let index = zti_dsl::build_index(&root_cow)?;
    tracing::info!(
        "{} symbols, {} edges, {} files",
        index.symbols.len(),
        index.edges.len(),
        index.files.len()
    );

    match command {
        DslCommands::FileTree { path_glob: _ } => {
            let file_indices: Vec<u16> = (0..index.files.len() as u16).collect();
            print!("{}", render_files_only(&index, &file_indices));
        }
        DslCommands::ProjectMap {
            language,
            path_glob: _,
            kinds,
            max_tokens,
        } => {
            let file_filter: Option<Vec<u16>> = language.as_ref().and_then(|l| {
                let lang = parse_language(l)?;
                Some(
                    index
                        .files
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| f.language == lang)
                        .map(|(i, _)| i as u16)
                        .collect(),
                )
            });
            let kind_filter: Option<Vec<zti_ts_core::types::Kind>> =
                kinds.as_ref().map(|k| parse_kinds(k));
            let renderer = DslRenderer::new(&index, max_tokens);
            print!(
                "{}",
                renderer.render(file_filter.as_deref(), kind_filter.as_deref())
            );
        }
        DslCommands::DepTree {
            id,
            direction,
            depth,
        } => {
            let renderer = AsciiTreeRenderer::new(&index);
            match direction.as_str() {
                "callers" => print!("{}", renderer.render_callers(id, depth)),
                "callees" => print!("{}", renderer.render_callees(id, depth, false)),
                _ => return Err(anyhow::anyhow!("direction must be 'callers' or 'callees'")),
            }
        }
        DslCommands::SymbolBody { id } => {
            let entries = zti_dsl::resolve_symbol_bodies(&index, &[id]);
            match entries.first() {
                Some(zti_common::dsl::SymbolBodyEntry::Ok {
                    kind_short,
                    symbol_id,
                    start_line,
                    end_line,
                    body,
                    ..
                }) => {
                    println!("{}#{} : {}-{}", kind_short, symbol_id, start_line, end_line);
                    println!("{}", body);
                }
                Some(zti_common::dsl::SymbolBodyEntry::Err { message, .. }) => {
                    return Err(anyhow::anyhow!("{}", message));
                }
                None => return Err(anyhow::anyhow!("Symbol {} not found", id)),
            }
        }
        DslCommands::SymbolBodies { ids } => {
            let entries = zti_dsl::resolve_symbol_bodies(&index, &ids);
            for entry in &entries {
                println!("{}\n---", entry);
            }
        }
        DslCommands::ChunkTrace => {
            let chunker = DslChunker::new(&index);

            let mut terminal_cache: HashMap<zti_tree_sitter::Language, Vec<u16>> =
                HashMap::with_capacity(4);
            for lang in index.files.iter().map(|f| f.language) {
                if terminal_cache.contains_key(&lang) {
                    continue;
                }
                let frontend = frontend_for(lang);
                let ts_lang = frontend.language();
                let names = frontend.config().terminal_node_kinds;
                let mut ids = Vec::with_capacity(names.len());
                for name in names {
                    let id = ts_lang.id_for_node_kind(name, true);
                    if id != 0 {
                        ids.push(id);
                    }
                }
                terminal_cache.insert(lang, ids);
            }

            eprintln!(
                "terminal_cache: {} languages, starting sequential trace for {} files",
                terminal_cache.len(),
                index.files.len(),
            );

            let sizing = zti_recursive_chunk::ChunkConfig {
                chunk_size: 2048,
                min_chunk_size: 512,
                chunk_overlap: 200,
            };

            let total = index.files.len();
            let mut total_chunks = 0usize;
            let mut total_sub = 0usize;
            let trace_start = Instant::now();

            for (i, file) in index.files.iter().enumerate() {
                let contents = match std::fs::read_to_string(&file.path) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!(
                            "DEBUG [{}/{}] {} - skip (read error: {})",
                            i + 1,
                            total,
                            file.path,
                            e
                        );
                        continue;
                    }
                };

                let f_start = Instant::now();
                let chunks = chunker.chunks_for_file(&file.path, &contents);
                let f_locate = f_start.elapsed();

                eprintln!(
                    "DEBUG [{}/{}] {} ({}B, {}) -> {} chunks in {:?}{}",
                    i + 1,
                    total,
                    file.path,
                    contents.len(),
                    file.language.as_str(),
                    chunks.len(),
                    f_locate,
                    if f_locate.as_millis() > 500 {
                        " WARN"
                    } else {
                        ""
                    },
                );

                let frontend = frontend_for(file.language);
                let ts_lang = frontend.language();
                let terminal_ids = terminal_cache
                    .get(&file.language)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);

                for (ci, chunk) in chunks.iter().enumerate() {
                    let c_start = Instant::now();
                    let sub = zti_recursive_chunk::split_text(
                        &chunk.body,
                        &sizing,
                        Some(&ts_lang),
                        terminal_ids,
                    );
                    let c_elapsed = c_start.elapsed();

                    if c_elapsed.as_millis() > 50 {
                        eprintln!(
                            "DEBUG   [{}/{}] sym={} kind={:?} body={}B -> {} sub in {:?}{}",
                            ci + 1,
                            chunks.len(),
                            chunk.sym_id,
                            chunk.kind,
                            chunk.body.len(),
                            sub.len(),
                            c_elapsed,
                            if c_elapsed.as_millis() > 500 {
                                " WARN"
                            } else {
                                ""
                            },
                        );
                    }

                    total_sub += sub.len();
                }

                total_chunks += chunks.len();

                let f_total = f_start.elapsed();
                if f_total.as_millis() > 1000 {
                    eprintln!("DEBUG   WARN: file took {:?}", f_total);
                }

                let _ = std::io::stderr().flush();
            }

            let elapsed = trace_start.elapsed();
            println!();
            println!("--- Chunk Trace Summary ---");
            println!(
                "Files: {total}, Chunks: {total_chunks}, Sub-chunks: {total_sub}, Time: {:.2}s",
                elapsed.as_secs_f64(),
            );
        }
    }

    Ok(())
}
