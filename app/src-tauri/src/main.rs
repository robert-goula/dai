// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("serve") {
        return dai_app::serve_daemon();
    }
    dai_app::run();
    Ok(())
}
