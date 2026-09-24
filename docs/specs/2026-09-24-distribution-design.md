# DAI distribution: installers, signing, updates, CLI on PATH

## Context

Today DAI runs from a source checkout: `cargo install --path crates/cli` for the CLI, and `bun tauri dev` for the app. To use it day to day on several machines, and eventually to share it, it needs:
- real installers for macOS, Windows, and Linux,
- the `dai` CLI on PATH (agents' MCP config points at it),
- updates that don't strand an old daemon.

Signing only matters for sharing. For personal use, unsigned builds work with one-time OS prompts.

**Prerequisite:** a git remote with the CI matrix green on all three OSes. Windows and Linux have never been compiled, and every step below builds on CI.

## Decisions

### Ship the CLI as a Tauri sidecar

The `dai` CLI is built from `crates/cli` and bundled with `bundle.externalBin` (the binary is named with its target triple, which Tauri requires). The alternative, making the app binary also act as the full CLI, is rejected: the release app is a Windows GUI-subsystem binary, so it can't write to a console, and a separate small binary keeps `dai` usable on headless machines. The app binary keeps its `serve` mode (used by autostart and by the app to start the daemon).

### Getting `dai` onto PATH

- **macOS:** an app action, **Install command-line tool**, symlinks `~/.local/bin/dai`, or `/usr/local/bin/dai` with an admin prompt, to the sidecar inside `DAI.app`. This is the VS Code `code` pattern. The symlink target survives app updates because the bundle path is stable.
- **Windows:** the NSIS installer adds the install folder (containing `dai.exe`) to the user PATH through an installer hook.
- **Linux:** the `.deb`/`.rpm` install `/usr/bin/dai`. The AppImage relies on the in-app action, the same as macOS.
- **MCP setup helper:** an app action shows or copies `claude mcp add dai -- <absolute path to dai> mcp` for the installed location.

### Version skew between the daemon and clients

After an update, a daemon started by the old version keeps running. Fix:
- `/api/health` already returns `version`.
- The client (CLI, `dai mcp`, the app) compares it with its own version. If the daemon is older, it asks it to shut down (`POST /api/shutdown`) and starts the new one.
- A newer daemon than the client is left alone (an old CLI talking to a new app is fine).

### Updates

`tauri-plugin-updater` with its own signing keypair, generated with `tauri signer generate`:
- The private key lives in CI secrets.
- The public key goes in `tauri.conf.json`.
- The endpoint is the `latest.json` that `tauri-action` publishes to GitHub Releases.

The app checks on start and from a "Check for updates" action, then downloads, installs, and restarts. The version-skew rule then restarts the daemon. The CLI sidecar updates with the app bundle.

Linux caveat: the updater only works for the AppImage. `.deb`/`.rpm` update through the package manager (or a manual download).

### OS code signing (optional until sharing)

- **macOS:** requires an Apple Developer Program membership (paid yearly) for Developer ID signing and notarization. `tauri-action` handles both with `APPLE_CERTIFICATE`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`. Tauri signs sidecars too. Unsigned builds work for personal use (right-click → Open, or clearing quarantine).
- **Windows:** Authenticode through a signing service, e.g. Azure Trusted Signing via `bundle.windows.signCommand`, or an OV certificate. Unsigned builds work but trigger SmartScreen warnings.
- **Linux:** no signing is needed for AppImage/deb/rpm.

## Release pipeline

- Pushing a `v*` tag triggers a GitHub Actions matrix: macOS arm64 and x64 (or universal), Windows x64, Ubuntu x64.
- Each job builds the CLI sidecar first, then runs `tauri-action` to build, sign (when secrets are present), and upload the installers plus the updater artifacts and `latest.json` to a draft release.
- **One version everywhere:** workspace crates, `tauri.conf.json`, and `app/package.json`, set by a small `scripts/bump-version` step so they can't drift.

## Phases

1. **D1: Remote and CI green.** Create the repo, push, and fix the Windows/Linux build issues CI surfaces.
2. **D2: CLI sidecar and PATH.** `externalBin` wiring, a build step that places the target-triple binary, Install command-line tool (macOS/Linux), the NSIS PATH hook (Windows), the MCP setup helper, and the version-skew restart in the client.
3. **D3: Unsigned release workflow.** Tag-triggered matrix build that publishes installers to a draft GitHub Release. Install it on your machines.
4. **D4: Updater.** Keypair, plugin, `latest.json`, and check/install UI. Verify an update from vN to vN+1 end to end, including the daemon restart.
5. **D5: Signing.** When sharing becomes a goal: Apple notarization and Windows signing secrets in CI.

## Decisions for you

- **Distribution scope:** personal machines only (D1–D4, no paid certificates), or shared publicly (adds D5 and its costs)? **Recommendation:** D1–D4 now, D5 when you want to share.
- **macOS architectures:** a universal binary (one download, bigger) or separate arm64/x64 builds? **Recommendation:** universal, which is simpler for a personal tool.
- **Package managers later** (Homebrew tap, winget, AUR): out of scope until D3 exists.

## Verification

- CI: all three OS jobs are green on every push (fmt, clippy, tests, frontend checks) before D2 starts.
- D2: on each OS, `dai --version` works from a new terminal after installing; `claude mcp add` with the helper's command works. Starting an old daemon and then running a new CLI gets the daemon restarted with the new version (checked through `/api/health`).
- D3: installing from each installer launches the app, the daemon starts, and search works. Start-at-login still works from the installed location.
- D4: install vN, publish vN+1, and the app offers the update, installs it, and relaunches. `dai --version` and the daemon report vN+1.
- D5: a signed macOS build opens without the Gatekeeper prompt, and a signed Windows installer shows the publisher in the UAC dialog.
