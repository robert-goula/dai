import { describe, expect, it } from "vitest";
import { progressLabel } from "./useDaiEvents";

describe("progressLabel", () => {
  it("shows percent when the size is known", () => {
    expect(progressLabel({ stage: "download", bytes: 425, total: 1000 })).toBe("Downloading 42%");
  });
  it("falls back to megabytes", () => {
    expect(progressLabel({ stage: "download", bytes: 12_400_000, total: null })).toBe(
      "Downloading 12 MB",
    );
  });
  it("labels indexing", () => {
    expect(progressLabel({ stage: "index", bytes: 0, total: null })).toBe("Indexing…");
  });
});
