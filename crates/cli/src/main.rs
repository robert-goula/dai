use std::time::Instant;

use anyhow::Result;
use clap::{Parser, Subcommand};
use dai_core::paths;
use dai_daemon::client::Client;

/// DAI (Docs AI): local documentation for you and your agents.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon in the foreground (HTTP API + MCP at /mcp).
    Serve {
        /// Defaults to $DAI_PORT, then 4747.
        #[arg(long)]
        port: Option<u16>,
    },
    /// Run an MCP server over stdio for agents (starts the daemon if needed).
    Mcp,
    /// Stop the running daemon.
    Stop,
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = paths::home()?;

    let dai = match cli.command {
        Command::Serve { port } => {
            let port = port
                .unwrap_or_else(|| dai_daemon::port_from_env().unwrap_or(dai_daemon::DEFAULT_PORT));
            return dai_daemon::server::serve(&home, port).await;
        }
        Command::Mcp => return dai_daemon::mcp::serve_stdio(&home).await,
        Command::Stop => {
            match Client::new(&home)?.shutdown().await {
                Ok(()) => println!("daemon stopped"),
                Err(_) => println!("daemon is not running"),
            }
            return Ok(());
        }
        command => (command, Client::connect(&home).await?),
    };

    match dai {
        (Command::Catalog { filter, refresh }, c) => {
            let filter = filter.map(|f| f.to_lowercase());
            let installed: Vec<String> = c.docsets().await?.into_iter().map(|d| d.id).collect();
            for d in c.catalog(refresh).await? {
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
        (Command::Install { slugs }, c) => {
            for slug in slugs {
                install(&c, &slug).await?;
            }
        }
        (Command::Update { slugs }, c) => {
            let slugs = if slugs.is_empty() {
                c.outdated(true).await?.into_iter().map(|d| d.id).collect()
            } else {
                slugs
            };
            if slugs.is_empty() {
                println!("everything is up to date");
            }
            for slug in slugs {
                install(&c, &slug).await?;
            }
        }
        (Command::Remove { ids }, c) => {
            for id in ids {
                let removed = c.remove(&id).await?;
                println!(
                    "{id}: {}",
                    if removed { "removed" } else { "not installed" }
                );
            }
        }
        (Command::List, c) => {
            for d in c.docsets().await? {
                println!("{:<32} {:<28} {:<10} {}", d.id, d.name, d.version, d.source);
            }
        }
        (
            Command::Search {
                query,
                docsets,
                limit,
                json,
            },
            c,
        ) => {
            let started = Instant::now();
            let hits = c.search(&query.join(" "), &docsets, limit).await?;
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
        (
            Command::Show {
                docset,
                path,
                offset,
                max_chars,
            },
            c,
        ) => match c.get_doc(&docset, &path, offset, max_chars).await? {
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
        (Command::Serve { .. } | Command::Mcp | Command::Stop, _) => unreachable!(),
    }
    Ok(())
}

async fn install(c: &Client, slug: &str) -> Result<()> {
    let started = Instant::now();
    eprintln!("installing {slug}...");
    let ds = c.install(slug).await?;
    println!(
        "{} {} installed in {:.1?}",
        ds.id,
        ds.version,
        started.elapsed()
    );
    Ok(())
}
