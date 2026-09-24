/** Viewer iframe URL for a page; a `#anchor` in `path` becomes the fragment. */
export function contentUrl(base: string, docset: string, path: string): string {
  const hash = path.indexOf("#");
  const page = hash === -1 ? path : path.slice(0, hash);
  const anchor = hash === -1 ? "" : path.slice(hash + 1);
  const encoded = page.split("/").map(encodeURIComponent).join("/");
  // Passed through as-is: Dash anchors are already percent-encoded.
  const fragment = anchor ? `#${anchor}` : "";
  return `${base}/content/${encodeURIComponent(docset)}/${encoded}${fragment}`;
}
