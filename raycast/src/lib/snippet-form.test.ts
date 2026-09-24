import { expect, test } from "vitest";
import { languageOptions, parseTags } from "./snippet-form";

test("parseTags trims, drops empties, dedupes", () => {
  expect(parseTags("react, hooks,,  , react ")).toEqual(["react", "hooks"]);
  expect(parseTags("")).toEqual([]);
});

test("languageOptions sorts and dedupes existing languages", () => {
  expect(languageOptions(["tsx", "rust", "", "tsx"], "")).toEqual(["rust", "tsx"]);
});

test("languageOptions offers new typed text first", () => {
  expect(languageOptions(["rust"], " zig ")).toEqual(["zig", "rust"]);
});

test("languageOptions doesn't duplicate an existing language", () => {
  expect(languageOptions(["Rust"], "rust")).toEqual(["Rust"]);
});
