/** `"react, hooks,,  "` → `["react", "hooks"]`, deduped. */
export function parseTags(text: string): string[] {
  return [
    ...new Set(
      text
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean),
    ),
  ];
}

/** Dropdown choices: languages already in use (sorted), plus what's being typed if it's new. */
export function languageOptions(existing: string[], typed: string): string[] {
  const langs = [...new Set(existing.map((l) => l.trim()).filter(Boolean))].sort();
  const t = typed.trim();
  return t && !langs.some((l) => l.toLowerCase() === t.toLowerCase()) ? [t, ...langs] : langs;
}
