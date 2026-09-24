import { expect, test } from "vitest";
import type { DocPage, Snippet } from "../daemon";
import { pageDetail, snippetDetail, snippetMarkdown } from "./markdown";

const page = (over: Partial<DocPage>): DocPage => ({
  docset: "react",
  path: "reference/react/useeffect",
  url: "",
  markdown: "# useEffect",
  offset: 0,
  total_chars: 11,
  next_offset: null,
  ...over,
});

test("whole page is shown as-is", () => {
  expect(pageDetail(page({}))).toBe("# useEffect");
});

test("truncated page gets a footer", () => {
  const md = pageDetail(page({ markdown: "a".repeat(25), total_chars: 100, next_offset: 25 }));
  expect(md.startsWith("a".repeat(25))).toBe(true);
  expect(md).toContain("first 25%");
  expect(md).toContain("Open in DAI");
});

const snippet = (over: Partial<Snippet>): Snippet => ({
  id: "debounce",
  title: "Debounce",
  language: "ts",
  tags: [],
  description: "",
  code: "const x = 1;",
  notes: "",
  created: "",
  updated: "",
  ...over,
});

test("snippet detail is the fenced code, after any description", () => {
  expect(snippetDetail(snippet({}))).toBe("```ts\nconst x = 1;\n```");
  expect(snippetDetail(snippet({ description: "Waits." }))).toBe("Waits.\n\n```ts\nconst x = 1;\n```");
});

test("fence outgrows backticks in the code", () => {
  expect(snippetDetail(snippet({ language: "md", code: "```js\nx\n```" }))).toBe("````md\n```js\nx\n```\n````");
});

test("snippet markdown has title, description, code, and notes", () => {
  expect(snippetMarkdown(snippet({ description: "Waits.", notes: "See lodash.\n" }))).toBe(
    "# Debounce\n\nWaits.\n\n```ts\nconst x = 1;\n```\n\nSee lodash.",
  );
});
