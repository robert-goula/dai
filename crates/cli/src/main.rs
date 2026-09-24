use std::time::Instant;

use anyhow::Result;
use clap::{Parser, Subcommand};
use dai_core::{generate, paths};
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
    /// List available docsets (DevDocs and Dash/Zeal).
    Catalog {
        /// Only show docsets whose name or slug contains this.
        filter: Option<String>,
        /// Re-download the catalog instead of using the cached copy.
        #[arg(long)]
        refresh: bool,
    },
    /// Download and index docsets by id (e.g. `react`, `python~3.12`, `dash:React`).
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
        /// Prefer docs matching this project's dependency versions.
        #[arg(short, long)]
        project: Option<std::path::PathBuf>,
    },
    /// Show how a project's dependencies map to installed docsets.
    Project {
        /// Project folder (defaults to the current directory).
        path: Option<std::path::PathBuf>,
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
    /// Work with saved code snippets.
    #[command(subcommand)]
    Snippet(SnippetCommand),
    /// Build a markdown docset for a library without a (current) docset.
    Generate {
        #[command(subcommand)]
        source: GenerateSource,
        /// Docset name (id becomes `md:<name>`). Derived from the source if omitted.
        #[arg(long, global = true)]
        name: Option<String>,
    },
    /// Find Context7 library ids (for `dai generate context7`).
    Context7 {
        name: String,
        /// What you're looking for, to rank results.
        query: Vec<String>,
    },
}

#[derive(Subcommand)]
enum GenerateSource {
    /// From a site's llms-full.txt / llms.txt (site root or the file's URL).
    Llms { url: String },
    /// From a git repo's README and docs folders.
    Repo {
        url: String,
        /// Branch or tag (defaults to the repo's default branch).
        #[arg(long = "ref")]
        git_ref: Option<String>,
    },
    /// From markdown files in a local folder.
    Dir { path: std::path::PathBuf },
    /// From Context7 results for a library id (see `dai context7`).
    Context7 {
        library_id: String,
        /// Topics to fetch, one page each (repeatable). Defaults to a general set.
        #[arg(short, long = "topic")]
        topics: Vec<String>,
    },
}

#[derive(Subcommand)]
enum SnippetCommand {
    /// List snippets, or search them when a query is given.
    List {
        query: Vec<String>,
        #[arg(short, long)]
        language: Option<String>,
        #[arg(short, long)]
        tag: Option<String>,
    },
    /// Print a snippet (use --code to print just the code, e.g. for piping).
    Show {
        id: String,
        #[arg(long)]
        code: bool,
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
                if filter.as_ref().is_some_and(|f| {
                    !d.id.to_lowercase().contains(f) && !d.name.to_lowercase().contains(f)
                }) {
                    continue;
                }
                let mark = if installed.contains(&d.id) { "*" } else { " " };
                println!(
                    "{mark} {:<36} {:<36} {:<12} {}",
                    d.id, d.name, d.version, d.source
                );
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
                project,
            },
            c,
        ) => {
            let started = Instant::now();
            let project = project.map(std::path::absolute).transpose()?;
            let hits = c
                .search(&query.join(" "), &docsets, project.as_deref(), limit)
                .await?;
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
        (
            Command::Snippet(SnippetCommand::List {
                query,
                language,
                tag,
            }),
            c,
        ) => {
            let found = c
                .snippets(&query.join(" "), language.as_deref(), tag.as_deref(), 200)
                .await?;
            for s in found {
                let tags = if s.tags.is_empty() {
                    String::new()
                } else {
                    format!("#{}", s.tags.join(" #"))
                };
                println!("{:<36} {:<12} {:<40} {tags}", s.id, s.language, s.title);
            }
        }
        (Command::Snippet(SnippetCommand::Show { id, code }), c) => match c.snippet(&id).await? {
            Some(s) if code => println!("{}", s.code),
            Some(s) => print!("{}", dai_core::snippets::render(&s)),
            None => anyhow::bail!("no snippet `{id}`"),
        },
        (Command::Generate { source, name }, c) => {
            let source = match source {
                GenerateSource::Llms { url } => generate::Source::Llms { url },
                GenerateSource::Repo { url, git_ref } => generate::Source::Repo { url, git_ref },
                GenerateSource::Dir { path } => generate::Source::Dir {
                    // The daemon resolves paths, so make it absolute here.
                    path: std::path::absolute(&path)?.to_string_lossy().into_owned(),
                },
                GenerateSource::Context7 { library_id, topics } => {
                    generate::Source::Context7 { library_id, topics }
                }
            };
            let started = Instant::now();
            eprintln!("generating from {}...", source.origin());
            let ds = c.generate(name.as_deref(), &source).await?;
            println!(
                "{} {} generated in {:.1?}",
                ds.id,
                ds.version,
                started.elapsed()
            );
        }
        (Command::Context7 { name, query }, c) => {
            for lib in c.context7_libraries(&name, &query.join(" ")).await? {
                println!(
                    "{:<40} {:<24} {:>8} tokens  {}",
                    lib.id, lib.title, lib.total_tokens, lib.description
                );
            }
        }
        (Command::Project { path }, c) => {
            let path = std::path::absolute(path.unwrap_or_else(|| ".".into()))?;
            print!(
                "{}",
                dai_daemon::mcp::project_summary(&c.project(&path).await?)
            );
        }
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
