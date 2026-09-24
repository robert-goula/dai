//! Markdown docsets' pages: MDX cleanup, HTML rendering for the viewer, and
//! the title/section entries that make them searchable by name.

use std::sync::LazyLock;

use regex::Regex;

use crate::store::Entry;

/// Rendering options: GFM, GitHub-style heading ids (matched by `entries`),
/// and raw HTML (sanitized afterwards).
fn options() -> comrak::Options<'static> {
    let mut o = comrak::Options::default();
    o.extension.table = true;
    o.extension.strikethrough = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.header_id_prefix = Some(String::new());
    o.render.r#unsafe = true;
    o
}

/// Sanitized HTML: generated docsets come from arbitrary sites and repos, so
/// scripts, iframes, event handlers, etc. are removed. Heading `id`s and code
/// `class`es are kept.
pub fn to_html(markdown: &str) -> String {
    static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
        let mut b = ammonia::Builder::default();
        b.add_generic_attributes(["id", "class"]);
        b
    });
    SANITIZER
        .clean(&comrak::markdown_to_html(markdown, &options()))
        .to_string()
}

/// Makes MDX readable as plain markdown: drops top-level `import`/`export`
/// lines and `{/* comments */}`, and strips component tags (`<Tabs>`,
/// `<Callout type="x">`, `<Foo />`) while keeping their children. Code fences
/// are left alone.
pub fn clean_mdx(text: &str) -> String {
    static COMPONENT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"</?[A-Z][A-Za-z0-9_.]*(\s[^<>]*)?/?>").unwrap());
    static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{/\*.*?\*/\}").unwrap());

    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence {
            if t.starts_with("import ") || t.starts_with("export ") {
                continue;
            }
            let cleaned = COMMENT.replace_all(line, "");
            let cleaned = COMPONENT.replace_all(&cleaned, "");
            if cleaned.trim().is_empty() && !line.trim().is_empty() {
                continue;
            }
            out.push_str(&cleaned);
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The page's first `# ` heading, if any.
pub fn title(markdown: &str) -> Option<String> {
    headings(markdown)
        .find(|(level, _)| *level == 1)
        .map(|(_, t)| t)
}

/// Search entries for a page: the page itself (`Guide`) and its `##`/`###`
/// headings (`Section`, `path#anchor`, anchors matching `to_html`).
pub fn entries(path: &str, markdown: &str, fallback_title: &str) -> Vec<Entry> {
    let page_title = title(markdown).unwrap_or_else(|| fallback_title.to_string());
    let mut anchors = comrak::Anchorizer::new();
    let mut out = vec![Entry {
        name: page_title.clone(),
        path: path.to_string(),
        kind: "Guide".into(),
    }];
    for (level, text) in headings(markdown) {
        // Anchorize every heading, in order, so duplicate suffixes line up.
        let anchor = anchors.anchorize(&text);
        if (2..=3).contains(&level) {
            out.push(Entry {
                name: format!("{text} - {page_title}"),
                path: format!("{path}#{anchor}"),
                kind: "Section".into(),
            });
        }
    }
    out
}

/// ATX headings outside code fences, with inline markup stripped.
fn headings(markdown: &str) -> impl Iterator<Item = (usize, String)> + '_ {
    static INLINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[`*_]|\[([^\]]*)\]\([^)]*\)").unwrap());
    let mut in_fence = false;
    markdown.lines().filter_map(move |line| {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            return None;
        }
        if in_fence {
            return None;
        }
        let level = t.bytes().take_while(|b| *b == b'#').count();
        if !(1..=6).contains(&level) || !t[level..].starts_with(' ') {
            return None;
        }
        let text = t[level..].trim().trim_end_matches('#').trim();
        Some((level, INLINE.replace_all(text, "$1").into_owned()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdx_components_and_imports_are_stripped() {
        let mdx = "import { Tabs } from 'x';\n\n# Title\n\n<Tabs items={['a']}>\n<Tab>\nInside **tab**\n</Tab>\n</Tabs>\n\
                   {/* hidden */}\n<Callout type=\"warn\">Careful</Callout>\n\n```jsx\nimport React from 'react';\n<App />\n```\n";
        let md = clean_mdx(mdx);
        assert!(!md.contains("import { Tabs }"));
        assert!(!md.contains("<Tab") && !md.contains("hidden"));
        assert!(md.contains("Inside **tab**") && md.contains("Careful"));
        assert!(
            md.contains("import React from 'react';\n<App />"),
            "code fences untouched: {md}"
        );
    }

    #[test]
    fn section_anchors_match_rendered_ids() {
        let md = "# Guide\n\n## Getting `started`\n\ntext\n\n## Getting started\n\n### [Linked](x.md) part\n\n```\n## not a heading\n```\n";
        let entries = entries("guide.md", md, "fallback");
        let html = to_html(md);
        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "guide.md",
                "guide.md#getting-started",
                "guide.md#getting-started-1",
                "guide.md#linked-part"
            ]
        );
        assert_eq!(entries[0].name, "Guide");
        assert_eq!(entries[1].name, "Getting started - Guide");
        for e in &entries[1..] {
            let anchor = e.path.split('#').nth(1).unwrap();
            assert!(
                html.contains(&format!("id=\"{anchor}\"")),
                "{anchor} missing in {html}"
            );
        }
    }

    #[test]
    fn html_rendering_filters_scripts() {
        let html = to_html(
            "<details><summary>More</summary>ok</details>\n\n<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n",
        );
        assert!(html.contains("<details>"));
        assert!(!html.contains("<script") && !html.contains("onerror"));
    }
}
