import { describe, expect, test } from "vitest";
import { DEFAULT_PORT, dataDir, parsePort, snippetPath } from "./discovery";

describe("dataDir", () => {
  test("macOS default", () => {
    expect(dataDir({ platform: "darwin", homedir: "/Users/me" })).toBe(
      "/Users/me/Library/Application Support/dai",
    );
  });

  test("preference overrides", () => {
    expect(dataDir({ platform: "darwin", homedir: "/Users/me", override: "/tmp/dai" })).toBe(
      "/tmp/dai",
    );
  });

  test("empty override falls back to default", () => {
    expect(dataDir({ platform: "darwin", homedir: "/Users/me", override: "" })).toBe(
      "/Users/me/Library/Application Support/dai",
    );
  });

  test("other platforms aren't supported yet", () => {
    expect(() => dataDir({ platform: "win32", homedir: "C:\\Users\\me" })).toThrow(/unsupported/);
  });
});

describe("parsePort", () => {
  test("reads the port", () => {
    expect(parsePort('{"port":5151,"pid":42}')).toBe(5151);
  });

  test.each([
    undefined,
    "",
    "not json",
    "null",
    '{"pid":1}',
    '{"port":"80"}',
    '{"port":0}',
    '{"port":70000}',
  ])("falls back for %j", (input) => {
    expect(parsePort(input)).toBe(DEFAULT_PORT);
  });
});

test("snippetPath", () => {
  expect(snippetPath("/Users/me/.config/dai/snippets", "rust/retry")).toBe(
    "/Users/me/.config/dai/snippets/rust/retry.md",
  );
});
