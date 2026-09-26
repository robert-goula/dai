# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

DAI (Docs AI): local, offline docs (DevDocs, Dash/Zeal, generated markdown) and code snippets, served to a desktop app, a CLI, and agents over MCP. See README.md for user-facing behavior.

## Commands

Rust workspace (root) + Tauri/React app (`app/`, uses **bun**). CI runs exactly these:

```shell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p dai-core normalize::        # one crate / test-name filter

cd app
bun run typecheck    # tsc --noEmit
bun run lint         # oxlint src
bunx oxfmt --check src
bun run test         # vitest run
bunx vitest run src/project.test.ts       # single test file
```

- Clippy/tests over the workspace compile `app/src-tauri`, and `tauri-build` needs `app/dist` to exist. If it's missing: `mkdir -p app/dist`.
- Rust tests are inline `#[cfg(test)]` modules; frontend tests sit next to the source as `*.test.ts`.
- A running daemon keeps serving old code after a rebuild. `dai stop` so the next command spawns the new one.
- `DAI_HOME`, `DAI_PORT`, `DAI_SNIPPETS_DIR` isolate a test run from the real data dir / port 4747 / snippets.

## Architecture

```
crates/core    dai-core: domain + storage. No async, no HTTP server.
crates/daemon  dai-daemon: axum HTTP API (/api), MCP (/mcp), event stream, HTTP client
crates/cli     the `dai` binary (clap), a thin client over the daemon
app/src-tauri  desktop shell; the app binary doubles as the daemon (`<app> serve`)
app/src        React UI (TanStack Query, CSS modules)
docs/specs     design docs for planned work (hybrid search, Raycast, distribution)
```

**One daemon owns the data.** `dai serve` opens the `Library` and is the only writer. Everything else is a client: the CLI, `dai mcp` (stdio MCP server that proxies to the daemon), and the Tauri app. `daemon::client::Client::connect` spawns the daemon if `/api` health fails, then waits for it. The daemon writes `<home>/daemon.json` (port, pid) so clients find it; auth is a bearer token persisted in `<home>/token`.

**`dai_core::Library`** (`library.rs`) is the facade for install/update/remove/search/read. Storage under `DAI_HOME`:
- `meta.db` (SQLite, `store.rs`): docsets, entries, and every page as both HTML and markdown.
- `index/` (tantivy, `index.rs`): one index for entries (named symbols), heading-scoped markdown chunks, and snippets. Search boosts exact symbol names.
- `docsets/dash/`, `docsets/md/<slug>/` (raw docset files; generated ones carry a `docset.toml` manifest recording the `generate::Source` so `update` re-runs it).

Ingest per source: `devdocs.rs`, `dash.rs`, `generate.rs` (`md:` ids: llms.txt, git repo, dir, Context7). All funnel through `normalize.rs` (HTML → markdown, markdown → chunks). `project.rs` parses package.json / Cargo / go.mod / Python manifests to map dependencies to installed docset versions. `snippets.rs` treats the snippets folder as the source of truth (markdown + YAML frontmatter); the daemon watches it and reindexes.

**MCP has two transports, one tool set.** `mcp.rs` (`DaiMcp`) runs over `backend::Backend`: `Local` in-process inside the daemon (streamable HTTP at `/mcp`), or `Remote(Client)` for `dai mcp` over stdio. New tools must work through both. Tool output is compact text/markdown, not JSON, to save tokens.

**Desktop app.** The webview never talks to the daemon directly. All calls go through Tauri commands in `app/src-tauri/src/lib.rs` (keeps the token out of the webview), and daemon SSE events (`DaiEvent` from `/api/events`) are re-emitted as the `dai-event` Tauri event. `open_in_app` from an agent either pushes an `Open` event to a connected app or launches it via a `dai://open` deep link. Pages render in an iframe using the HTML shell in `daemon/src/viewer.rs`.

**Keep types in sync by hand.** `app/src/api.ts` mirrors Rust serde types (`Docset`, `CatalogEntry`, `Hit`, `generate::Source`, `ProjectReport`, ...). Changing one side means changing the other, plus the Tauri command and `Client` method if it's a new endpoint.
