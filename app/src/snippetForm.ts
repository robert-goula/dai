import type { Snippet, SnippetInput } from "./api";

/** Form fields; tags are edited as one comma/space-separated string. */
export type SnippetDraft = Omit<SnippetInput, "tags"> & { tags: string };

export const emptyDraft: SnippetDraft = {
  title: "",
  language: "",
  tags: "",
  description: "",
  code: "",
  notes: "",
};

export function toDraft(s: Snippet): SnippetDraft {
  return { ...s, tags: s.tags.join(", ") };
}

export function toInput(d: SnippetDraft): SnippetInput {
  return { ...d, tags: parseTags(d.tags) };
}

export function parseTags(tags: string): string[] {
  return [
    ...new Set(
      tags
        .split(/[,\s]+/)
        .map((t) => t.replace(/^#/, "").trim().toLowerCase())
        .filter(Boolean),
    ),
  ];
}
