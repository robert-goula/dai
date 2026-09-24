//! HTML shell for pages shown in the desktop app's viewer iframe.

const STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #ffffff; --fg: #1f2328; --muted: #59636e; --border: #d1d9e0;
  --code-bg: #f6f8fa; --link: #0969da; --accent-bg: #ddf4ff;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #16181d; --fg: #e6e8eb; --muted: #9198a1; --border: #30363d;
    --code-bg: #1f2329; --link: #58a6ff; --accent-bg: #16273d;
  }
}
* { box-sizing: border-box; }
html { background: var(--bg); color: var(--fg); }
body {
  margin: 0; padding: 24px 32px 64px;
  font: 15px/1.6 system-ui, -apple-system, "Segoe UI", sans-serif;
}
main { max-width: 860px; margin: 0 auto; overflow-wrap: anywhere; }
h1, h2, h3, h4 { line-height: 1.25; margin: 1.6em 0 0.6em; }
h1 { font-size: 1.9em; margin-top: 0.2em; }
h2 { font-size: 1.4em; padding-bottom: 0.3em; border-bottom: 1px solid var(--border); }
a { color: var(--link); text-decoration: none; }
a:hover { text-decoration: underline; }
code, pre { font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
code { background: var(--code-bg); padding: 0.15em 0.35em; border-radius: 4px; }
pre { background: var(--code-bg); padding: 12px 14px; border-radius: 6px; overflow-x: auto; }
pre code { background: none; padding: 0; }
table { border-collapse: collapse; margin: 1em 0; display: block; overflow-x: auto; }
th, td { border: 1px solid var(--border); padding: 6px 10px; text-align: left; vertical-align: top; }
blockquote, .note, ._note { margin: 1em 0; padding: 8px 14px; background: var(--accent-bg); border-radius: 6px; }
img { max-width: 100%; }
dt { font-weight: 600; margin-top: 1em; }
"#;

/// Reports the current page to the parent window and routes external links
/// out to the system browser (the parent handles both messages).
const SCRIPT: &str = r#"
parent.postMessage({ type: "dai:page", docset: DAI_DOCSET, path: DAI_PATH, title: document.title }, "*");
document.addEventListener("click", (e) => {
  const a = e.target.closest("a[href]");
  if (!a || a.origin === location.origin) return;
  e.preventDefault();
  parent.postMessage({ type: "dai:external", url: a.href }, "*");
});
"#;

pub fn wrap(docset: &str, path: &str, html: &str) -> String {
    let title = path.rsplit('/').next().unwrap_or(path);
    let js_str = |s: &str| serde_json::to_string(s).expect("string serializes");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>{STYLE}</style></head><body><main>{html}</main>\
         <script>const DAI_DOCSET = {}; const DAI_PATH = {};{SCRIPT}</script></body></html>",
        js_str(docset),
        js_str(path),
        title = html_escape(title),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
