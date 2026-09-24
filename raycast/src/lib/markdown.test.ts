import { expect, test } from "vitest";
import type { DocPage } from "../daemon";
import { pageDetail } from "./markdown";

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
