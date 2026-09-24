import { describe, expect, it } from "vitest";
import { contentUrl } from "./content";

describe("contentUrl", () => {
  const base = "http://127.0.0.1:4747";

  it("keeps path segments and moves the anchor to the fragment", () => {
    expect(contentUrl(base, "rust", "std/vec/struct.vec#method.push")).toBe(
      "http://127.0.0.1:4747/content/rust/std/vec/struct.vec#method.push",
    );
  });

  it("encodes docset ids and segments", () => {
    expect(contentUrl(base, "python~3.12", "library/a b")).toBe(
      "http://127.0.0.1:4747/content/python~3.12/library/a%20b",
    );
  });
});
