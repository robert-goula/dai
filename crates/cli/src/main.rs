use std::time::Instant;

use anyhow::Result;
use clap::{Parser, Subcommand};
use dai_core::{Library, paths};

/// DAI (Docs AI): local documentation for you and your agents.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List available DevDocs docsets.
    Catalog {
        /// Only show docsets whose name or slug contains this.
        filter: Option<String>,
        /// Re-download the catalog instead of using the cached copy.
        #[arg(long)]
        refresh: bool,
    },
    /// Download and index DevDocs docsets by slug (e.g. `react`, `python~3.12`).
    Install { slugs: Vec<String> },
    /// Update the given docsets, or every outdated one.
    Update { slugs: Vec<String> },
    /// Remove installed docsets.
    Remove { ids: Vec<String> },
    /// List installed docsets.
    List,
    /// Search installed docs.
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        /// Limit to these docsets (repeatable).
        #[arg(short, long = "docset")]
        docsets: Vec<String>,
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print a page as markdown.
    Show {
        docset: String,
        path: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        max_chars: Option<usize>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut lib = Library::open(&paths::home()?)?;

    match cli.command {
        Command::Catalog { filter, refresh } => {
            let filter = filter.map(|f| f.to_lowercase());
            let installed: Vec<String> = lib.installed()?.into_iter().map(|d| d.id).collect();
            for d in lib.catalog(refresh)? {
                if filter
                    .as_ref()
                    .is_some_and(|f| !d.slug.contains(f) && !d.name.to_lowercase().contains(f))
                {
                    continue;
                }
                let mark = if installed.contains(&d.slug) {
                    "*"
                } else {
                    " "
                };
                println!("{mark} {:<32} {:<28} {}", d.slug, d.name, d.release);
            }
        }
        Command::Install { slugs } => {
            for slug in slugs {
                install(&mut lib, &slug)?;
            }
        }
        Command::Update { slugs } => {
            let slugs = if slugs.is_empty() {
                lib.outdated(true)?.into_iter().map(|d| d.id).collect()
            } else {
                slugs
            };
            if slugs.is_empty() {
                println!("everything is up to date");
            }
            for slug in slugs {
                install(&mut lib, &slug)?;
            }
        }
        Command::Remove { ids } => {
            for id in ids {
                let removed = lib.remove(&id)?;
                println!(
                    "{id}: {}",
                    if removed { "removed" } else { "not installed" }
                );
            }
        }
        Command::List => {
            for d in lib.installed()? {
                println!("{:<32} {:<28} {:<10} {}", d.id, d.name, d.version, d.source);
            }
        }
        Command::Search {
            query,
            docsets,
            limit,
            json,
        } => {
            let started = Instant::now();
            let hits = lib.search(&query.join(" "), &docsets, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
                return Ok(());
            }
            for h in &hits {
                let what = if h.kind == "entry" {
                    format!("[{}]", h.entry_type)
                } else {
                    h.heading.clone()
                };
                println!(
                    "{:>6.2}  {}  {}  {}  ({})",
                    h.score, h.docset, h.name, what, h.path
                );
                if !h.snippet.is_empty() {
                    println!("        {}", h.snippet.replace('\n', " "));
                }
            }
            eprintln!("{} hits in {:.1?}", hits.len(), started.elapsed());
        }
        Command::Show {
            docset,
            path,
            offset,
            max_chars,
        } => match lib.get_doc(&docset, &path, offset, max_chars)? {
            Some(page) => {
                println!("{}", page.markdown);
                if let Some(next) = page.next_offset {
                    eprintln!(
                        "-- {next}/{} chars, continue with --offset {next}",
                        page.total_chars
                    );
                }
            }
            None => anyhow::bail!("no page `{path}` in `{docset}`"),
        },
    }
    Ok(())
}

fn install(lib: &mut Library, slug: &str) -> Result<()> {
    let started = Instant::now();
    eprintln!("installing {slug}...");
    let ds = lib.install(slug)?;
    println!(
        "{} {} installed in {:.1?}",
        ds.id,
        ds.version,
        started.elapsed()
    );
    Ok(())
}
