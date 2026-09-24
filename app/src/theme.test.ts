import { describe, expect, it } from "vitest";
import { resolveScheme } from "./theme";

describe("resolveScheme", () => {
  it("follows the system only when the theme is system", () => {
    expect(resolveScheme("system", true)).toBe("dark");
    expect(resolveScheme("system", false)).toBe("light");
    expect(resolveScheme("light", true)).toBe("light");
    expect(resolveScheme("dark", false)).toBe("dark");
  });
});
