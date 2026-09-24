import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { getPreferenceValues } from "@raycast/api";
import { dataDir, parsePort } from "./lib/discovery";

// Copied from app/src/api.ts (and `DocPage` from crates/core/src/library.rs).

export type Docset = {
  id: string;
  name: string;
  source: string;
  version: string;
  release: string;
  mtime: number;
  installed_at: number;
};

export type Hit = {
  docset: string;
  kind: "entry" | "chunk";
  name: string;
  entry_type: string;
  path: string;
  heading: string;
  snippet: string;
  score: number;
};

export type DocPage = {
  docset: string;
  path: string;
  /** Upstream URL; empty when the source has none. */
  url: string;
  markdown: string;
  offset: number;
  total_chars: number;
  next_offset: number | null;
};

export type Snippet = {
  /** Relative path without `.md`, e.g. `rust/retry`. */
  id: string;
  title: string;
  language: string;
  tags: string[];
  description: string;
  code: string;
  notes: string;
  created: string;
  updated: string;
};

export type SnippetInput = Pick<Snippet, "title" | "language" | "tags" | "description" | "code" | "notes">;

export type Info = { version: string; snippets_dir: string };

/** The daemon isn't listening (or DAI has never run, so there's no token). */
export class DaemonDownError extends Error {
  constructor() {
    super("DAI service isn't running");
  }
}

function home(): string {
  const { dataDir: override } = getPreferenceValues<{ dataDir?: string }>();
  return dataDir({ platform: process.platform, homedir: homedir(), override });
}

async function readOptional(path: string): Promise<string | undefined> {
  try {
    return await readFile(path, "utf8");
  } catch {
    return undefined;
  }
}

async function connection(): Promise<{ base: string; token: string }> {
  const dir = home();
  const [info, token] = await Promise.all([
    readOptional(join(dir, "daemon.json")),
    readOptional(join(dir, "token")),
  ]);
  if (!token) throw new DaemonDownError();
  return { base: `http://127.0.0.1:${parsePort(info)}`, token: token.trim() };
}

async function send(path: string, init: RequestInit): Promise<Response> {
  const { base, token } = await connection();
  try {
    return await fetch(base + path, {
      ...init,
      headers: { ...init.headers, Authorization: `Bearer ${token}` },
    });
  } catch (e) {
    if ((e as { cause?: { code?: string } }).cause?.code === "ECONNREFUSED") throw new DaemonDownError();
    throw e;
  }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  let res = await send(path, init);
  // The token file may have been replaced since we read it; re-reading it is `send`'s job.
  if (res.status === 401) res = await send(path, init);
  if (!res.ok) throw new Error((await res.text()) || `${res.status} ${res.statusText}`);
  return (await res.json()) as T;
}

export const api = {
  info: () => request<Info>("/api/info"),
  docsets: (signal?: AbortSignal) => request<Docset[]>("/api/docsets", { signal }),
  search: (q: string, docsets: string[], signal?: AbortSignal) =>
    request<Hit[]>(`/api/search?${new URLSearchParams({ q, docsets: docsets.join(","), limit: "50" })}`, { signal }),
  doc: (docset: string, path: string, maxChars: number, signal?: AbortSignal) =>
    request<DocPage>(`/api/doc?${new URLSearchParams({ docset, path, max_chars: String(maxChars) })}`, { signal }),
  snippets: (q: string, signal?: AbortSignal) =>
    request<Snippet[]>(`/api/snippets?${new URLSearchParams({ q })}`, { signal }),
  createSnippet: (input: SnippetInput) =>
    request<Snippet>("/api/snippets", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    }),
  deleteSnippet: (id: string) =>
    request<boolean>(`/api/snippets/${id.split("/").map(encodeURIComponent).join("/")}`, { method: "DELETE" }),
  open: (docset: string, path: string) =>
    request<"shown" | "launched">("/api/open", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ docset, path }),
    }),
};
