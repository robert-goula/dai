import type { DocPage } from "../daemon";

/** Detail-pane markdown for a (possibly truncated) page window. */
export function pageDetail(page: DocPage): string {
  if (page.next_offset === null) return page.markdown;
  const pct = Math.round((page.markdown.length / page.total_chars) * 100);
  return `${page.markdown}\n\n---\n\n*Showing the first ${pct}% of the page. Open in DAI to read the rest.*`;
}
