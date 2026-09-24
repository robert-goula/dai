# DAI (Docs AI): local documentation for you and your agents

DAI keeps documentation for your languages, frameworks, and libraries locally on your machine.  It is searchable in three ways:

- a **desktop app** (macOS, Windows, Linux) for quick reference
- an **MCP server** so coding agents can search and read the same docs offline,
- a **CLI** (`dai`) for the managing everything from the terminal.

Use docs from [DevDocs](https://devdocs.io) and the Dash/Zeal catalogs, so you don't have to maintain them yourself.  For libraries without one, or with a stale one, DAI generates docsets from `llms.txt` in a git repo, a local folder, or [Context7](https://context7.com).

It also stores your **code snippets** as plain markdown files, available to you and your agents.

## Quick start

```shell
# Install the CLI (from a checkout of this repo)
cargo install --path crates/cli

# Add some docs
dai install react rust dash:TypeScript
dai search useEffect
dai show react reference/react/useeffect

# Give your agents access (all projects)
claude mcp add --scope user dai -- dai mcp
```

The CLI starts the background service (the "daemon") on first use. `dai stop` stops it.

## Docsets

| Source | Ids look like | How to add |
|---|---|---|
| DevDocs (836 docsets) | `react`, `react~18`, `python~3.12` | `dai install react~18` |
| Dash/Zeal (981: official, user-contributed, cheatsheets) | `dash:React`, `dash:React@18.3.1` | `dai install dash:React@18.3.1` |
| Generated (markdown) | `md:<name>` | `dai generate …` (see below) |

- `dai catalog [filter]` lists what's available, and `dai list` shows what's installed.
- `dai update` updates everything that's outdated. Pinned versions (`@1.2.3`) are left alone.
- Several versions can be installed side by side (e.g. `react`, `react~18`, `dash:React@18.3.1`). Project-aware search (below) picks the right one.

### Generate docs

For libraries without a (current) docset. Each generator records how it ran, so `dai update md:<name>` re-runs it.

From a site's `llms-full.txt` / `llms.txt`:

```shell
dai generate llms https://hono.dev
dai generate llms https://tanstack.com/query/latest/llms.txt --name "tanstack query"
```

Generate documents from a repository (README plus `docs/`-style folders):

```shell
dai generate repo https://github.com/TanStack/query.git --name "tanstack query"
dai generate repo https://github.com/sveltejs/svelte --ref svelte@4.2.19 --name svelte-4
```

Specify the docs path from the repo:

```shell
git clone --depth 1 https://github.com/TanStack/router.git ~/repos/tanstack-router
dai generate dir ~/repos/tanstack-router/docs/start --name "tanstack start"
```

From Context7 (one page per topic):

```shell
dai context7 zustand persist          # find the library id
dai generate context7 /pmndrs/zustand -t "persist middleware" -t "slices pattern"
```

`.mdx` files are supported: imports and components are stripped, and their text is kept. Generating from a repo needs `git` on your PATH.

## Using DAI from agents (MCP)

### Set up

`dai mcp` is an MCP server over stdio. It starts the DAI service in the background if it isn't running, and connects to it. Install the CLI first (`cargo install --path crates/cli`), then register it:

```shell
# Available in every project (recommended)
claude mcp add --scope user dai -- dai mcp

# Or only in the current project
claude mcp add dai -- dai mcp
```

Without `--scope user`, Claude Code registers the server for the current folder only (local scope), so other projects won't see it. Other MCP clients use the same command: `dai` with the argument `mcp`, or the absolute path from `which dai` if the client doesn't inherit your shell's PATH.

### Check it's working

```shell
claude mcp list        # should show: dai: dai mcp - ✔ Connected
dai list               # docsets the tools can see; empty means install some first
```

In a Claude Code session, `/mcp` lists the server and its tools. Try "search the docs for useEffect".

### Troubleshooting

- **Not listed in another project:** it was added with local scope. Run `claude mcp remove dai -s local`, then add it again with `--scope user`.
- **Fails to connect:** run `dai mcp` in a terminal. It should wait silently for input (Ctrl+C to exit). "command not found" means `~/.cargo/bin` isn't on the PATH Claude Code sees; register the absolute path instead. If it reports the service didn't start, check `<data dir>/daemon.log`. A common cause is another process on port 4747 (set `DAI_PORT`).
- **Tools behave like an older version:** a service started by an older build is still running. Run `dai stop`; the next call starts the new one.
- **Connected but no results:** nothing is installed yet (`dai install react`), or the docs are for a different version than your project (pass `project_path`, or run `dai project`).

Tools:

| Tool | What it does |
|---|---|
| `search_docs` | Ranked search. Symbols (`useEffect`, `Vec::push`) match API entries first. Pass `project_path` to prefer the project's dependency versions. |
| `get_doc` | A page as markdown, paged for long pages. |
| `list_docsets` | What's installed. |
| `resolve_project_versions` | How a project's dependencies map to installed docsets, plus what to install. |
| `search_snippets` / `get_snippet` | Your saved snippets, with code. |
| `save_snippet` | Save a new snippet (only when you ask; never overwrites). |
| `open_in_app` | Show a page to you in the DAI app. |

The daemon also serves MCP over streamable HTTP at `http://127.0.0.1:4747/mcp`. It needs the bearer token from `<data dir>/token`.

## Project versions

DAI reads `package.json` (plus `node_modules`), `Cargo.toml` and `Cargo.lock` (including workspaces), `go.mod`, and `pyproject.toml`/`requirements.txt`, then matches dependencies to installed docsets by name and version:

```shell
dai project                    # covered / different version / suggested installs
dai search useState --project .
```

With a project, searches skip installed docsets for *other* versions of your dependencies when a matching one is installed. The app's Search tab has the same option ("Match a project's versions…").

## Snippets

Snippets are markdown files: YAML frontmatter, the code in the first fenced block, then optional notes. Put them in folders if you like (ids become `rust/retry-with-backoff`). Edits made outside DAI (your editor, `git pull`) are picked up automatically.

```shell
dai snippet list [query] [-l rust] [-t async]
dai snippet show rust/retry-with-backoff --code | pbcopy
```

Default folder: `~/.config/dai/snippets` (see Configuration). It's a good candidate for a git repo.

## Desktop app

```shell
cd app
bun install
bun tauri dev
```

- **Search:** type-ahead with ⌘K / Ctrl+K and ↑/↓/Enter, a docset filter, and an optional project to match versions. "Ask Context7" is there for anything not installed.
- **Viewer:** Back/Forward and a Contents panel. External links open in your browser.
- **Snippets:** search, edit, copy, delete.
- **Docsets:** install (with version picker and progress), update, remove, and generate (llms.txt / git repo / folder / Context7).
- **Theme:** System / Light / Dark. DevDocs and generated pages follow it; Dash pages keep their own styling.
- **Start DAI service at login:** runs only the background service at login, so agents can use DAI without the window open.

The app starts the daemon itself (the app binary doubles as `serve`), so it works without the CLI installed.

## Configuration

| Setting | Default | Override |
|---|---|---|
| Data (docsets, index, token) | `~/Library/Application Support/dai` (macOS) or the platform data dir | `DAI_HOME` |
| Daemon port | `4747` | `DAI_PORT` |
| Snippets folder | `~/.config/dai/snippets` | `snippets_dir` in `~/.config/dai/config.toml`, or `DAI_SNIPPETS_DIR` |
| Context7 API key | none (works rate-limited) | `context7_api_key` in `config.toml`, or `CONTEXT7_API_KEY` |

```toml
# ~/.config/dai/config.toml
snippets_dir = "~/code/snippets"
context7_api_key = "ctx7sk-…"
```

After rebuilding or updating DAI, run `dai stop` so the next command starts the new daemon.

## CLI reference

```text
Usage: dai <COMMAND>

Commands:
  serve     Run the daemon in the foreground (HTTP API + MCP at /mcp)
  mcp       Run an MCP server over stdio for agents (starts the daemon if needed)
  stop      Stop the running daemon
  catalog   List available docsets (DevDocs and Dash/Zeal)
  install   Download and index docsets by id (e.g. `react`, `python~3.12`, `dash:React`)
  update    Update the given docsets, or every outdated one
  remove    Remove installed docsets
  list      List installed docsets
  search    Search installed docs
  project   Show how a project's dependencies map to installed docsets
  show      Print a page as markdown
  snippet   Work with saved code snippets
  generate  Build a markdown docset for a library without a (current) docset
  context7  Find Context7 library ids (for `dai generate context7`)
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## Development

```text
crates/core     docsets, ingest, markdown, search index (tantivy), snippets, projects
crates/daemon   HTTP API, MCP server, event stream, client
crates/cli      the `dai` binary
app/            desktop app: React UI (app/src) + Tauri shell (app/src-tauri)
docs/specs/     design spec and plans (Raycast, hybrid search, distribution)
```

Checks (the same ones CI runs):

```shell
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cd app && bun run typecheck && bun run lint && bunx oxfmt --check src && bun run test
```
