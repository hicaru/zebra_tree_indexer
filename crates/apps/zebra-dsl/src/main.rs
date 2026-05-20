use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use zti_dsl::chunking::DslChunker;
use zti_dsl::render::dsl::{DslRenderer, render_files_only};
use zti_dsl::render::tree::AsciiTreeRenderer;
use zti_tree_sitter::{parse_kinds, parse_language};

#[derive(Parser)]
#[command(name = "zebra-dsl", version, about = "DSL graph dump for debugging")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long, help = "Project root path")]
    root: PathBuf,
}

#[derive(Subcommand)]
enum Commands {
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
    #[command(about = "Show chunks in embed or display format")]
    Chunks {
        #[arg(short, long, help = "File path relative to root (omit for all files)")]
        file: Option<String>,
        #[arg(
            long,
            help = "Use embed format (header-body-header) instead of display format"
        )]
        embed: bool,
        #[arg(long, help = "Show template (manifest + legend) without chunking")]
        template: bool,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = Cli::parse();
    let root_str = cli.root.canonicalize()?.to_string_lossy().to_string();

    let index = zti_dsl::build_index(&root_str)?;
    tracing::info!(
        "{} symbols, {} edges, {} files",
        index.symbols.len(),
        index.edges.len(),
        index.files.len()
    );

    match cli.command {
        Commands::FileTree { path_glob: _ } => {
            let file_indices: Vec<u16> = (0..index.files.len() as u16).collect();
            print!("{}", render_files_only(&index, &file_indices));
        }
        Commands::ProjectMap {
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
        Commands::DepTree {
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
        Commands::SymbolBody { id } => {
            let sym = index
                .symbols
                .get(id as usize)
                .ok_or_else(|| anyhow::anyhow!("Symbol {} not found", id))?;
            let file = index
                .files
                .get(sym.file_idx as usize)
                .ok_or_else(|| anyhow::anyhow!("File not found for symbol {}", id))?;
            let content = std::fs::read_to_string(&file.path)?;
            let range = zti_common::line_byte_range(&content, sym.line, sym.end_line);
            println!(
                "// File: {} | Lines: {}-{}",
                file.path, sym.line, sym.end_line
            );
            println!("{}", &content[range]);
        }
        Commands::Chunks {
            file,
            embed,
            template,
        } => {
            let manifest = zti_dsl::chunking::find_manifest(&cli.root);
            let chunker = DslChunker::new(&index, manifest.as_deref());

            if template {
                print!("{}", chunker.manifest_header());
                return Ok(());
            }

            let files: Vec<(String, String)> = match &file {
                Some(path) => {
                    let full = cli.root.join(path);
                    vec![(full.display().to_string(), std::fs::read_to_string(&full)?)]
                }
                None => index
                    .files
                    .iter()
                    .filter_map(|f| {
                        std::fs::read_to_string(&f.path)
                            .ok()
                            .map(|c| (f.path.clone(), c))
                    })
                    .collect(),
            };

            for (label, content) in &files {
                for chunk in chunker.chunks_for_file(label, content) {
                    if embed {
                        println!("{}", chunk.embed_text());
                    } else {
                        println!("{}", chunk.display_text());
                    }
                    println!("---SEPARATOR---");
                }
            }
        }
    }

    Ok(())
}
