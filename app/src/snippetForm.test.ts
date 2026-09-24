import { describe, expect, it } from "vitest";
import { parseTags } from "./snippetForm";

describe("parseTags", () => {
  it("splits on commas and spaces, strips #, dedupes, lowercases", () => {
    expect(parseTags(" React, #hooks  react\tAsync,, ")).toEqual(["react", "hooks", "async"]);
  });
  it("handles empty input", () => {
    expect(parseTags("  ")).toEqual([]);
  });
});
