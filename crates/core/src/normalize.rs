//! HTML -> markdown, and markdown -> heading-scoped chunks.

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Heading trail, e.g. "useEffect > Usage > Connecting to a server".
    pub heading: String,
    pub body: String,
}

pub fn html_to_markdown(html: &str) -> Result<String> {
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "nav", "aside", "footer", "svg", "noscript", "button",
        ])
        .build();
    let md = converter.convert(html)?;
    // Heading permalinks come through as `[](#anchor "Link for …")`.
    static EMPTY_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\[\]\([^)]*\)"#).unwrap());
    Ok(EMPTY_LINK.replace_all(&md, "").into_owned())
}

/// The main content of a full HTML page (site captures in Dash docsets carry
/// navigation, sidebars, and footers): the first `<article>`, `<main>`, or
/// `[role=main]`, else the whole `<body>`.
pub fn main_content(html: &str) -> String {
    static SELECTORS: LazyLock<Vec<scraper::Selector>> = LazyLock::new(|| {
        ["article", "main", "[role=main]", "body"]
            .into_iter()
            .map(|s| scraper::Selector::parse(s).unwrap())
            .collect()
    });
    let doc = scraper::Html::parse_document(html);
    SELECTORS
        .iter()
        .find_map(|s| doc.select(s).next())
        .map_or_else(|| html.to_string(), |el| el.inner_html())
}

/// Splits markdown at ATX headings (ignoring ones inside code fences).
/// Sections with no body text are dropped; their heading stays in the trail.
pub fn chunk_markdown(md: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut trail: Vec<(usize, String)> = Vec::new();
    let mut body = String::new();
    let mut in_fence = false;

    for line in md.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence && let Some((level, text)) = parse_heading(t) {
            flush(&mut chunks, &trail, &mut body);
            trail.retain(|(l, _)| *l < level);
            trail.push((level, text.to_string()));
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    flush(&mut chunks, &trail, &mut body);
    chunks
}

fn flush(chunks: &mut Vec<Chunk>, trail: &[(usize, String)], body: &mut String) {
    let text = body.trim();
    if !text.is_empty() {
        let heading = trail
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" > ");
        chunks.push(Chunk {
            heading,
            body: text.to_string(),
        });
    }
    body.clear();
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&level) || !line[level..].starts_with(' ') {
        return None;
    }
    Some((level, line[level..].trim().trim_end_matches('#').trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_by_heading_trail() {
        let md = "intro\n# A\none\n## B\ntwo\n# C\nthree\n";
        let got: Vec<_> = chunk_markdown(md)
            .into_iter()
            .map(|c| (c.heading, c.body))
            .collect();
        assert_eq!(
            got,
            vec![
                ("".into(), "intro".into()),
                ("A".into(), "one".into()),
                ("A > B".into(), "two".into()),
                ("C".into(), "three".into()),
            ]
        );
    }

    #[test]
    fn ignores_headings_in_code_fences() {
        let md = "# A\n```sh\n# not a heading\n```\n";
        let chunks = chunk_markdown(md);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].body.contains("# not a heading"));
    }

    #[test]
    fn empty_sections_keep_trail() {
        let chunks = chunk_markdown("# A\n## B\nbody\n");
        assert_eq!(
            chunks,
            vec![Chunk {
                heading: "A > B".into(),
                body: "body".into()
            }]
        );
    }

    #[test]
    fn converts_html() {
        let md =
            html_to_markdown("<h1>Title</h1><p>Hello <code>x</code></p><script>bad()</script>")
                .unwrap();
        assert!(md.contains("# Title"));
        assert!(md.contains("`x`"));
        assert!(!md.contains("bad()"));
    }

    #[test]
    fn main_content_prefers_article_then_body() {
        let page = "<html><body><nav>menu</nav><main><aside>toc</aside>\
                    <article><h1>Title</h1><p>text</p></article></main><footer>f</footer></body></html>";
        let main = main_content(page);
        assert!(main.contains("<h1>Title</h1>") && !main.contains("menu") && !main.contains("toc"));

        let plain = main_content("<html><body><p>just body</p></body></html>");
        assert_eq!(plain.trim(), "<p>just body</p>");
    }

    #[test]
    fn strips_empty_permalinks() {
        let md = html_to_markdown(
            r##"<h2>Reference<a href="#reference" title="Link for Reference"></a></h2>"##,
        )
        .unwrap();
        assert_eq!(md.trim(), "## Reference");
    }
}
