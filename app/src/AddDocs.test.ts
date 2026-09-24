import { describe, expect, it } from "vitest";
import { toSource } from "./AddDocs";

describe("toSource", () => {
  it("omits an empty git ref", () => {
    expect(toSource("repo", "https://github.com/a/b", "")).toEqual({
      kind: "repo",
      url: "https://github.com/a/b",
    });
  });
  it("splits Context7 topics", () => {
    expect(toSource("context7", "/a/b", "routing, , forms ")).toEqual({
      kind: "context7",
      library_id: "/a/b",
      topics: ["routing", "forms"],
    });
  });
});
