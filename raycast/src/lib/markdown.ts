import type { DocPage, Snippet } from "../daemon";

/** Detail-pane markdown for a (possibly truncated) page window. */
export function pageDetail(page: DocPage): string {
  if (page.next_offset === null) return page.markdown;
  const pct = Math.round((page.markdown.length / page.total_chars) * 100);
  return `${page.markdown}\n\n---\n\n*Showing the first ${pct}% of the page. Open in DAI to read the rest.*`;
}

/** A fence longer than any backtick run in `code`, so the block can't end early. */
function fenced(code: string, language: string): string {
  const longest = Math.max(0, ...(code.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}${language}\n${code}\n${fence}`;
}

/** Detail-pane markdown for a snippet: description, then the code. */
export function snippetDetail(s: Snippet): string {
  return [s.description, fenced(s.code, s.language)].filter(Boolean).join("\n\n");
}

/** The whole snippet as a standalone markdown document. */
export function snippetMarkdown(s: Snippet): string {
  return [`# ${s.title}`, s.description, fenced(s.code, s.language), s.notes.trim()].filter(Boolean).join("\n\n");
}
