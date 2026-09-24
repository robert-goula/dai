import { describe, expect, it } from "vitest";
import type { ProjectReport } from "./api";
import { folderName, projectStatus } from "./project";

const dep = (name: string, spec: string, resolved: string | null = null) => ({
  ecosystem: "npm",
  name,
  spec,
  resolved,
});

describe("projectStatus", () => {
  it("summarizes matches, mismatches, and suggestions", () => {
    const report: ProjectReport = {
      root: "/p",
      covered: [
        {
          dependency: dep("react", "^18.2.0", "18.3.1"),
          installed: [{ id: "react~18", version: "18", matches: true }],
        },
        {
          dependency: dep("vue", "^2.7.0"),
          installed: [{ id: "vue~3", version: "3.5", matches: false }],
        },
      ],
      uncovered: [dep("vite", "^6.0.0")],
      suggestions: [
        { dependency: "vue", id: "vue~2", version: "2.7" },
        { dependency: "vite", id: "vite~6", version: "6.3.5" },
      ],
    };
    const { summary, details } = projectStatus(report);
    expect(summary).toBe("1 matched · 1 other version · 2 to install");
    expect(details).toEqual([
      "✓ react 18.3.1: react~18",
      "✗ vue ^2.7.0: only vue~3 (3.5) — install vue~2",
      "+ vite: available as vite~6",
    ]);
  });
});

describe("folderName", () => {
  it("handles both separators", () => {
    expect(folderName("/Users/me/code/app/")).toBe("app");
    expect(folderName("C:\\code\\site")).toBe("site");
  });
});
