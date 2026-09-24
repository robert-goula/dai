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
| IDE snippets | Raycast extension, in a separate later plan. No global hotkey for now. |
| Agent → you | MCP tool `open_in_app` opens the app to a specific doc |

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
- **Lifecycle:** the app and `dai mcp` both start the daemon if it isn't running (a second daemon fails to bind the port, so duplicates cannot run). The app has an opt-in "start at login" toggle using Tauri's autostart plugin. It registers `<app> serve`, so only the background service starts at login, not the window. No launchd/systemd/Windows service in v1.
- **Storage:** platform dirs from the `directories` crate. `meta.db` (SQLite via rusqlite) holds docset, entry, and version metadata. Dash's `docSet.dsidx` is already SQLite, so it maps over naturally. Raw content lives on disk and the tantivy index sits beside it. Page contents in `meta.db` are zstd-compressed (level 3; older plain-text rows still read), and the database uses incremental auto-vacuum so removed or replaced docsets free their space. React + Rust + Python dropped from 185MB to 48MB.

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
- Inside a Dash docset: `Contents/Resources/docSet.dsidx` has the `searchIndex(name, type, path)` table (or, for Apple-style docsets, Core Data `ZTOKEN` tables, read with Zeal's joins and Apple type codes mapped to names), plus `Documents/`. Paths may carry `#//dash_ref…` anchors (already percent-encoded). Extracted docsets live under `<data dir>/docsets/dash/<name>/` and the viewer serves them from disk; only the markdown goes into `meta.db`. Markdown comes from each page's `<article>`/`<main>`, falling back to `<body>`.

### Normalization and indexing

- The HTML→markdown conversion (e.g. `htmd`) strips nav and chrome, then chunks at headings. Each chunk records `docset, version, entry name, type, path#anchor, heading trail, markdown`.
- tantivy fields: `name` (boosted, symbol-aware tokenizer so `useEffect`, `Vec::push`, and `os.path.join` all match), `heading`, `body`, `docset`, `version`, `kind` (doc/snippet), `type`.
- Search runs as prefix/symbol match on names, then BM25 on the body. Results come back in under ~20ms for the app's type-ahead.

### MCP tools (rmcp, stdio + streamable HTTP)

- `list_docsets`: installed docsets with versions.
- `search_docs(query, docsets?, project_path?, limit)`: chunk hits with source path/URL. With `project_path`, installed docsets for other versions of the project's dependencies are skipped when a matching version is installed. The output starts with notes on which versions were used, or warns about mismatches with the `dai install` fix.
- `get_doc(docset, path, anchor?)`: full markdown for one entry, with a size limit and pagination.
- `search_snippets(query?, language?, tag?)` (returns code inline) / `get_snippet(id)` / `save_snippet(title, code, language, …)`. Saving only creates new files; it never overwrites.
- `resolve_project_versions(project_path)`: reads package.json (plus `node_modules` versions), Cargo.toml (workspace members plus Cargo.lock), go.mod, and pyproject.toml/requirements.txt, including language/runtime versions. Maps them to installed docsets (matches / different version / unknown) and suggests catalog ids to install (a DevDocs pin like `react~18`, else a Dash version like `dash:React@18.3.1`). Also available as `dai project [path]`.
- `open_in_app(docset, path)`: the daemon emits an SSE event, and the app focuses and navigates. If the app isn't running, the daemon launches it via the `dai://open?...` deep link (Tauri deep-link plugin). The tool description says to use it only when the user asks to see something.
- Context7 is **not** exposed over MCP (agents have their own Context7 MCP).

### Desktop app (Tauri + React)

- Search-first UI: a type-ahead box, a results list filtered by docset, a doc viewer, and a table of contents (headings reported by the iframe's shell script; ids are added where pages lack them). Search can take a project folder (a folder picker) to use its dependency versions.
- Doc viewer: an iframe of the daemon's `/content/<docset>/<path>`, sandboxed and allow-listed in the CSP. DevDocs HTML is wrapped in our stylesheet, Dash pages are served from disk, and generated markdown pages are rendered by the daemon (comrak, GitHub-style heading ids) and sanitized (ammonia). This replaces the planned React markdown renderer, so there's one viewer path for `open_in_app` and anchors.
- Docset manager: browse the catalog (DevDocs + Dash, filterable by source), install, remove, update, and "update all". It shows stale/outdated state and progress over SSE.
- Snippets: list, search, filter by language, edit, tag, copy code, delete.
- "Not found locally" → a **Context7** panel, opened from Search with the current query: pick a library, read the results (rendered with react-markdown), and optionally "Save as docset" (a Context7 snapshot). The daemon proxies Context7. The API key is optional (`$CONTEXT7_API_KEY` or `context7_api_key` in `~/.config/dai/config.toml`); without one, Context7 is rate-limited.
- Deep-link handler for `dai://open`, plus a listener for SSE `open` events.

### Generated docsets (the markdown docset format)

`<data dir>/docsets/md/<slug>/` holds `docset.toml` (name, version, generated_at, and the `source` used to generate it) plus `pages/**.md(x)` and images. Ids are `md:<slug>`. The same format is used for llms.txt, git repos, local folders, and Context7 snapshots. Search entries are each page's title (`Guide`) and its `##`/`###` headings (`Section`, with anchors matching the rendered ids). `.mdx` files go through the MDX cleanup.

1. **llms.txt:** use `<site>/llms-full.txt` split into pages at `# ` headings. Otherwise use `llms.txt` plus the same-site pages it links to, one level deep, capped at 2,000 pages, fetched 8 at a time, with HTML pages converted.
2. **Repo:** `git clone --depth 1` (needs `git` on PATH) at an optional branch or tag. Collects the README plus `docs/`, `doc/`, `documentation/`, `website/docs/`, `content/docs/`, or else every markdown file. The version is the ref or the short commit.
3. **Context7 snapshot:** one page per topic (a default set if none is given), from the app panel or `dai generate context7 <id> -t <topic>`. `dai context7 <name>` finds library ids.
4. **Local folder:** every markdown file and image under a path (`dai generate dir <path>`).
Re-running a generator counts as an "update" (`dai update md:<slug>`, or Regenerate in the app). Generated docsets are never flagged as outdated automatically.

### Snippets

`<snippets dir>/**/<slug>.md`, with frontmatter `{title, language, tags, description, created, updated}` and the code in the first fenced block, plus optional notes. The folder is `$DAI_SNIPPETS_DIR`, else `snippets_dir` in `~/.config/dai/config.toml`, else `~/.config/dai/snippets`. A file watcher (`notify`) reloads and reindexes on change. Files without valid frontmatter still load, using the file name as the title. Snippets live in the search index under the `snippets` pseudo-docset, and doc searches skip it. Subfolders are read recursively (ids like `rust/retry`), hidden folders such as `.git` are skipped by both the loader and the watcher, and new snippets are created at the top level.

## Phases (each gets review + commit before the next)

0. **Scaffold:** `git init`, the cargo workspace, the three crates, CI config for fmt/clippy/test on mac, win, and linux, and a copy of this spec in `docs/specs/2026-09-24-dai-design.md`.
1. **Core + daemon + MCP (DevDocs only).** Catalog fetch, install/update of DevDocs docsets, normalize, index, search, CLI commands, the `serve` HTTP API, MCP `list_docsets`/`search_docs`/`get_doc` over stdio and HTTP. *Done when Claude Code can answer from local React/Rust docs through `dai mcp`.*
2. **Desktop app v1.** Search, viewer, docset manager, SSE progress, auto-starting the daemon, `open_in_app` + deep link.
3. **Dash/Zeal docsets.** Zeal catalog, streamed `.tgz` install with progress events, dsidx import, the same normalize/index pipeline.
4. **Snippets.** Store, watcher, index, MCP snippet tools (incl. `save_snippet`), CLI, app editor.
5. **Generation + Context7.** Markdown docset format; the llms.txt, repo, folder, and Context7 generators; `dai generate`; the app's Generate form and Context7 panel with snapshot-to-docset.
6. **Version awareness.** Manifest parsers, name and version matching, `resolve_project_versions`, `project_path` on `search_docs`, and side-by-side Dash versions (`dash:<name>@<version>`, with a version picker in the app). DevDocs already has versioned ids. Pinned docsets are never flagged as outdated. Dash archives get portable paths on extraction (e.g. `127.0.0.1:3000` → `127.0.0.1_3000`, for Windows), and symlinks/hardlinks are skipped.

Separate later plans: **Raycast extension** (a thin client over `/api`), **hybrid/semantic search**, **signed installers and auto-update** (Tauri updater, code signing).

## Risks and trade-offs

- **Dash HTML is noisy.** Markdown extraction quality varies by docset. The per-source stripping rules will need tuning, and the viewer falls back to the raw HTML.
- **Index size.** Installing many docsets could push the index into the GBs. Only the markdown is indexed, stored pages are compressed, and Dash HTML stays on disk.
- **Licensing.** DevDocs and Dash content is fine for local personal use. We don't build any redistribution or sharing of downloaded content.
- **Raycast** is Mac-first, so Linux users only get snippets in the app and CLI (`dai snippet show <id> --code`).
- **Context7** needs an API key and network access. It's app-only and optional.

## Verification

- `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` on core. Unit tests use fixture DevDocs `index.json`/`db.json` and a tiny Dash docset fixture. Search-ranking tests check symbol queries.
- Integration: start `dai serve`, install `rust` and `react` from DevDocs, then run `dai search useEffect` and time it.
- MCP: add `dai mcp` to Claude Code (`claude mcp add dai -- dai mcp`) and check that `search_docs` and `get_doc` return markdown. Also check against the MCP Inspector over streamable HTTP.
- App: `bun run typecheck`, the oxc lint, and vitest for UI logic. A manual pass on the installed app on macOS, plus Windows and Linux via CI builds. Have an agent call `open_in_app` and confirm the app focuses on the right doc, both with the app open and with it closed.
