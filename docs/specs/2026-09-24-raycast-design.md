# DAI Raycast extension

## Context

Snippets and doc lookups should be one keystroke away while you're coding in any editor, without switching to the DAI window. Raycast is the launcher you use, so the extension is a thin client over the DAI daemon's local HTTP API. It adds no index or storage of its own.

Scope: macOS first. Windows is supported by Raycast (`"platforms": ["macOS", "Windows"]`) and is cheap to add, but it's untested until DAI itself builds on Windows (see CI). Linux has no Raycast; there, the DAI app and CLI (`dai snippet show <id> --code`) cover this.

## Commands

| Command | Mode | What it does |
|---|---|---|
| **Search Docs** | view | Type-ahead over `GET /api/search` (throttled), with a docset filter dropdown from `GET /api/docsets`. The detail pane shows the selected page's markdown (`GET /api/doc`, first ~4k chars). |
| **Search Snippets** | view | Lists `GET /api/snippets?q=` with the code in a detail pane. The primary action pastes the code into the frontmost app. |
| **Save Snippet** | form | Title, language, tags, description, and code, prefilled from the selected text (else the clipboard). Posts to `POST /api/snippets`. |

Actions:
- **Search Docs:** Open in DAI (`POST /api/open`), Copy page as markdown, Copy/Open upstream URL (when `DocPage.url` is set), Copy docset/path.
- **Search Snippets:** Paste code (`Action.Paste`, primary), Copy code, Copy snippet as markdown, Open file in editor (`Action.Open`), Show in Finder, Delete (with confirmation, via `DELETE /api/snippets/*id`).

No Context7 or generation commands. Those stay in the app, which keeps the extension small.

## Talking to the daemon

- **Finding it:** read `<data dir>/daemon.json` (port) and `<data dir>/token`, which the extension can do directly from Node.
  - `<data dir>` defaults to the `directories` crate's location: `~/Library/Application Support/dai` on macOS (verified). On Windows it's under `%APPDATA%`; confirm the exact path the crate uses with an empty organization before enabling Windows.
  - A **"DAI data folder"** preference overrides it (Raycast doesn't see `$DAI_HOME`).
  - If `daemon.json` is missing, fall back to port 4747.
- **Auth:** `Authorization: Bearer <token>` on every request, the same as the CLI.
- **Daemon not running:** show an empty view with the message "DAI service isn't running" and two actions.
  - **Start DAI**: opens the `dai://` URL scheme. The app launches and starts the daemon. This needs `parse_open_url` to accept a bare `dai://`; today it only handles `dai://open?...`. It already just launches the app, so this is a one-line guard.
  - A hint pointing at the app's **Start DAI service at login** checkbox.
- **Errors:** a toast with the daemon's error text. 401 means the token file changed, so re-read it once and retry.

### Daemon additions (small)

1. `GET /api/info` → `{ version, snippets_dir }`, so the extension can open or reveal snippet files and show the version.
2. `Snippet` gains a `path` (absolute file path) in API responses, for Open in Editor and Show in Finder. The alternative is to compute it from `snippets_dir` + id. That's fine too, but the daemon already knows the path.
3. Accept a bare `dai://` in the app's deep-link handler (see above).

No new MCP tools and no index changes.

## Project layout

```
raycast/
  package.json        # Raycast manifest (commands, preferences, platforms)
  src/
    daemon.ts         # data-dir resolution, daemon.json/token, fetch wrapper, types
    search-docs.tsx
    search-snippets.tsx
    save-snippet.tsx
    lib/markdown.ts   # page/snippet → detail markdown (pure, unit-tested)
  assets/icon.png     # the 大 icon at 512px
```

Types for `Docset`, `Hit`, `DocPage`, and `Snippet` are copied from `app/src/api.ts`. A shared TS package isn't worth it for three types, but it's noted as possible duplication.

## Trade-offs and decisions for you

- **Store vs. private.** Publishing to the Raycast Store means review, a public repo path under `raycast/extensions`, and npm (the Store requires `package-lock.json`, not bun). Keeping it private means `ray develop`/`ray build` locally, with bun fine for dependencies. **Recommendation: private first**, and publish once DAI itself has an installer; a Store extension for an app nobody can install isn't useful.
- **Package manager.** Raycast's tooling expects npm. If private, bun works for installs, with `npx ray` for the Raycast CLI. If published later, switch that folder to npm.
- **Windows now or later.** Adding `"Windows"` to `platforms` is one line plus the data-dir path. It's untestable until DAI builds on Windows. **Recommendation: macOS only for now**, and add Windows after CI is green.

## Phases

1. **R1: Search Docs.** Manifest, `daemon.ts` (discovery, auth, retry on 401), list, detail pane, docset dropdown, Open in DAI, and the copy actions. Includes the not-running state and the `dai://` app change.
2. **R2: Search Snippets.** List, detail, and Paste/Copy. Adds `GET /api/info` and `Snippet.path`, then Open in Editor, Show in Finder, and Delete.
3. **R3: Save Snippet.** Form prefilled from the selection or clipboard. The language dropdown is seeded from the languages of existing snippets, and free text is allowed.
4. **R4: Polish.** Keyboard shortcuts for secondary actions, empty states, and an icon. Store submission only if you choose to publish.

## Verification

- Unit tests (vitest) for data-dir resolution per platform, `daemon.json` parsing and fallback, and the markdown builders.
- Daemon: an axum test (or live `curl`) for `GET /api/info` and `Snippet.path`. The app's deep-link parser gets a test for bare `dai://`.
- Manual in `ray develop`, with the daemon running:
  - Search `useEffect`, check the detail pane, and use Open in DAI (the app focuses on the page).
  - Search a snippet and paste it into an editor.
  - Save a snippet from selected text and see it in `dai snippet list`.
- With the daemon stopped, you see "DAI service isn't running". Start DAI launches the app, and the search works after it starts.
