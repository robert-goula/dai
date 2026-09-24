# DAI (Docs AI): local docs + snippets service, MCP server, and desktop app

## Context

You want something like Dash/Zeal, with three extras:

1. You can look up docs fast in a desktop app (Mac, Windows, Linux).
2. Agents can query the same docs through a local MCP server, so builds don't wait on web lookups.
3. It stores code snippets for both you and agents.

The docs come from standard upstream sources (DevDocs, Dash/Zeal feeds), so we don't maintain docsets ourselves. We generate docsets when a library isn't covered or its docs are stale. The app (not the agents) can reach out to Context7 for anything else.

Classification: **architectural, new project**. `projects/daimedia/docset/` is empty and not a git repo. This plan is the spec. On approval, step 0 copies it into `docs/specs/2026-09-24-dai-design.md` and commits it.

**Scope warning:** this is six subsystems. Asking for it all at once is too much for one pass, so it's split into phases. Each phase ships something usable and gets its own review before the next one starts. Raycast and semantic/hybrid search get **separate later plans**.

## Decisions (settled with you)

| Area | Decision |
|---|---|
| Language | Rust (cargo workspace) |
| Desktop | Tauri 2 + React/TS (bun, oxc for lint/format, vitest) |
| Sources | DevDocs + Dash docsets via the Zeal catalog (Kapeli official, user-contributed, cheatsheets). Dash ids are `dash:<name>`. |
| Canonical text | **Markdown.** Every source gets normalized into heading-chunked markdown for search and MCP. The original HTML is kept for viewing in the app. |
| MDX | Read as markdown: strip `import`/`export`, render unknown JSX as its children, map a few common components (Tabs, Callout/Admonition, CodeGroup). No runtime MDX compile or eval. |
| Search | BM25 via tantivy now. Hybrid/embeddings is a separate later plan. |
| Generation (v1) | llms.txt / llms-full.txt, source repo (README + `docs/**/*.md(x)`), Context7 snapshot (app-side) |
| Snippets | One `.md` file per snippet, with YAML frontmatter, in a watched folder you can git-sync |
| Versions | Manifest detection (package.json, Cargo.toml, go.mod, pyproject) in the MCP phase |
| IDE snippets | Raycast extension, in a separate later plan. The in-app global-hotkey palette covers Linux and Windows. |
| Agent → you | MCP tool `open_in_app` opens the app to a specific doc or snippet |

## Architecture

```
            ┌────────────── daid (daemon, Rust) ──────────────┐
agents ───▶ │ MCP: streamable HTTP  /mcp   (rmcp)                 │
 (stdio) ─▶ │ `dai mcp` = stdio shim → proxies to daemon        │
            │ HTTP API (axum)  /api/*  + /content/* (doc HTML)     │
app ──────▶ │ event stream /api/events (SSE) ← open_in_app, updates│
            │ core: catalog | ingest | normalize | index | snippets│
            └──── data dir: docsets/, index/ (tantivy), meta.db ───┘
```

- **One binary, `dai`**, with subcommands: `serve` (the daemon), `mcp` (stdio shim), `install/update/remove/list/search` (CLI). One artifact to ship per platform.
- **The daemon is the only index writer.** The app and the CLI are thin clients over `127.0.0.1:<port>`. The port and a random auth token go in `<data dir>/daemon.json`, and every HTTP/MCP call must present the token. That stops other local web pages from hitting the API.
- **Lifecycle:** the app and `dai mcp` both start the daemon if it isn't running (a second daemon fails to bind the port, so duplicates cannot run). The app has an opt-in "start at login" toggle using Tauri's autostart plugin (deferred past Phase 2). No launchd/systemd/Windows service in v1.
- **Storage:** platform dirs from the `directories` crate. `meta.db` (SQLite via rusqlite) holds docset, entry, and version metadata. Dash's `docSet.dsidx` is already SQLite, so it maps over naturally. Raw content lives on disk and the tantivy index sits beside it.

### Crates (workspace)

- `crates/core`: domain model, catalog clients, ingesters, HTML→markdown normalizer, tantivy indexing and search, snippet store, manifest/version resolver. No I/O frameworks, so it's testable in isolation.
- `crates/daemon`: axum server, rmcp MCP server, SSE events, job queue for installs and updates.
- `crates/cli`: the `dai` binary (clap), wiring subcommands to core and daemon.
- `app/`: Tauri 2 shell (`app/src-tauri`) + React UI (`app/src`). The app binary doubles as the daemon (`<app> serve`) so it can start one without a separate `dai` install.

### Source formats (verified 2026-09-24)

- DevDocs catalog: `https://devdocs.io/docs.json` has 836 docs, each with `{name, slug, version, release, mtime, db_size, links, attribution}`. Needs a User-Agent header.
- DevDocs per-doc: `https://documents.devdocs.io/<slug>/index.json` → `{entries:[{name,path,type}], types:[…]}`, and `db.json` → `{path: html}`. We update when `mtime` changes.
- Zeal catalog: `https://api.zealdocs.org/v1/docsets` has 981 Dash docsets (Kapeli official, `_Contrib` user-contributed, `_Cheatsheet`) in one list: `{name, title, versions[] (newest first; may contain nulls), size}`. It replaces reading the Kapeli XML feeds and user-contributed index separately.
- Dash download: `https://go.zealdocs.org/d/com.kapeli/<name>/latest` (or `/<version>`) redirects to the `.tgz`. Sizes go up to 3.3GB, so downloads stream to disk.
- Inside a Dash docset: `Contents/Resources/docSet.dsidx` has the `searchIndex(name, type, path)` table, plus `Documents/`. Paths may carry `#//dash_ref…` anchors (already percent-encoded). Extracted docsets live under `<data dir>/docsets/dash/<name>/` and the viewer serves them from disk; only the markdown goes into `meta.db`. Markdown comes from each page's `<article>`/`<main>`, falling back to `<body>`.

### Normalization and indexing

- The HTML→markdown conversion (e.g. `htmd`) strips nav and chrome, then chunks at headings. Each chunk records `docset, version, entry name, type, path#anchor, heading trail, markdown`.
- tantivy fields: `name` (boosted, symbol-aware tokenizer so `useEffect`, `Vec::push`, and `os.path.join` all match), `heading`, `body`, `docset`, `version`, `kind` (doc/snippet), `type`.
- Search runs as prefix/symbol match on names, then BM25 on the body. Results come back in under ~20ms for the app's type-ahead.

### MCP tools (rmcp, stdio + streamable HTTP)

- `list_docsets`: installed docsets with versions.
- `search_docs(query, docsets?, version?, project_path?, limit)`: chunk hits with source path/URL.
- `get_doc(docset, path, anchor?)`: full markdown for one entry, with a size limit and pagination.
- `search_snippets(query, language?, tags?)` / `get_snippet(id)`.
- `resolve_project_versions(project_path)`: maps a project's dependencies to installed docset versions (Phase 6).
- `open_in_app(docset, path, anchor?)` / `open_in_app(snippet_id)`: the daemon emits an SSE event, and the app focuses and navigates. If the app isn't running, the daemon launches it via the `dai://open?...` deep link (Tauri deep-link plugin). The tool description says to use it only when the user asks to see something.
- Context7 is **not** exposed over MCP (agents have their own Context7 MCP).

### Desktop app (Tauri + React)

- Search-first UI: a type-ahead box, a results list filtered by docset, a doc viewer, and a table of contents (TOC deferred past Phase 2).
- Doc viewer: an iframe of the daemon's `/content/<docset>/<path>` for sources that have HTML, sandboxed and allow-listed in the CSP. A markdown renderer (react-markdown + remark-gfm + our MDX shim) for generated and markdown docsets and snippets.
- Docset manager: browse the catalog (DevDocs + Dash, filterable by source), install, remove, update, and "update all". It shows stale/outdated state and progress over SSE.
- Snippets: list, edit, tag, and copy. The global-hotkey quick palette uses the Tauri global-shortcut plugin.
- "Not found locally" → a **Context7** panel: query Context7 using the API key from settings, then optionally "Save as docset" (a snapshot saved as a markdown docset).
- Deep-link handler for `dai://open`, plus a listener for SSE `open` events.

### Generated docsets (the markdown docset format)

A folder with `docset.toml` (name, version, source, generated_at, origin URL) and `**/*.md`. The same format is used for llms.txt, repo docs, Context7 snapshots, and any hand-written docs. Generators:

1. **llms.txt:** fetch `<site>/llms-full.txt` (fall back to `llms.txt` plus its linked pages) and split on headings.
2. **Repo:** shallow-clone a git URL at a tag or branch, then collect README + `docs/**/*.md(x)`. The version comes from the tag.
3. **Context7 snapshot:** run from the app with a list of topics, save the results.
Re-running a generator counts as an "update". The manager shows `generated_at` so staleness is visible.

### Snippets

`<snippets dir>/<slug>.md`, with frontmatter `{title, language, tags, description, created, updated}` and the body in a fenced block plus optional notes. A file watcher (`notify`) reindexes on change. The folder location is configurable so you can point it at a git repo.

## Phases (each gets review + commit before the next)

0. **Scaffold:** `git init`, the cargo workspace, the three crates, CI config for fmt/clippy/test on mac, win, and linux, and a copy of this spec in `docs/specs/2026-09-24-dai-design.md`.
1. **Core + daemon + MCP (DevDocs only).** Catalog fetch, install/update of DevDocs docsets, normalize, index, search, CLI commands, the `serve` HTTP API, MCP `list_docsets`/`search_docs`/`get_doc` over stdio and HTTP. *Done when Claude Code can answer from local React/Rust docs through `dai mcp`.*
2. **Desktop app v1.** Search, viewer, docset manager, SSE progress, auto-starting the daemon, `open_in_app` + deep link.
3. **Dash/Zeal docsets.** Zeal catalog, streamed `.tgz` install with progress events, dsidx import, the same normalize/index pipeline.
4. **Snippets.** Store, watcher, index, MCP snippet tools, app editor, hotkey palette.
5. **Generation + Context7.** Markdown docset format, the llms.txt and repo generators, the app's Context7 panel and snapshot-to-docset.
6. **Version awareness.** Manifest parsers, `resolve_project_versions`, `project_path` on `search_docs`, side-by-side installs of multiple versions.

Separate later plans: **Raycast extension** (a thin client over `/api`), **hybrid/semantic search**, **signed installers and auto-update** (Tauri updater, code signing).

## Risks and trade-offs

- **Dash HTML is noisy.** Markdown extraction quality varies by docset. The per-source stripping rules will need tuning, and the viewer falls back to the raw HTML.
- **Index size.** Installing many docsets could push the index into the GBs. Only the markdown is indexed, and the full HTML stays on disk.
- **Licensing.** DevDocs and Dash content is fine for local personal use. We don't build any redistribution or sharing of downloaded content.
- **Raycast** is Mac-first, so Linux users depend on the in-app palette.
- **Context7** needs an API key and network access. It's app-only and optional.

## Verification

- `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` on core. Unit tests use fixture DevDocs `index.json`/`db.json` and a tiny Dash docset fixture. Search-ranking tests check symbol queries.
- Integration: start `dai serve`, install `rust` and `react` from DevDocs, then run `dai search useEffect` and time it.
- MCP: add `dai mcp` to Claude Code (`claude mcp add dai -- dai mcp`) and check that `search_docs` and `get_doc` return markdown. Also check against the MCP Inspector over streamable HTTP.
- App: `bun run typecheck`, the oxc lint, and vitest for UI logic. A manual pass on the installed app on macOS, plus Windows and Linux via CI builds. Have an agent call `open_in_app` and confirm the app focuses on the right doc, both with the app open and with it closed.
