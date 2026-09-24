import { invoke } from "@tauri-apps/api/core";

export type Docset = {
  id: string;
  name: string;
  source: string;
  version: string;
  release: string;
  mtime: number;
  installed_at: number;
};

export type CatalogEntry = {
  /** Docset id once installed: a DevDocs slug or `dash:<name>`. */
  id: string;
  name: string;
  source: "devdocs" | "dash";
  version: string;
  /** Download size in bytes. */
  size: number;
  mtime: number;
  /** Older versions installable side by side as `<id>@<version>` (Dash only). */
  versions: string[];
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

export type Page = { docset: string; path: string };

/** How a markdown (`md:`) docset is generated; mirrors `generate::Source`. */
export type GenerateSource =
  | { kind: "llms"; url: string }
  | { kind: "repo"; url: string; git_ref?: string }
  | { kind: "dir"; path: string }
  | { kind: "context7"; library_id: string; topics: string[] };

/** Mirrors `dai_core::project::ProjectReport`. */
export type ProjectReport = {
  root: string;
  covered: {
    dependency: Dependency;
    /** Version matches first; `matches` is null when unknown. */
    installed: { id: string; version: string; matches: boolean | null }[];
  }[];
  uncovered: Dependency[];
  suggestions: { dependency: string; id: string; version: string }[];
};

export type Dependency = {
  ecosystem: string;
  name: string;
  spec: string;
  resolved: string | null;
};

export type Context7Library = {
  id: string;
  title: string;
  description: string;
  totalTokens: number;
  totalSnippets: number;
  versions: string[];
};

export type Snippet = {
  /** File stem in the snippets folder. */
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

export type SnippetInput = Pick<
  Snippet,
  "title" | "language" | "tags" | "description" | "code" | "notes"
>;

/** Emitted by the Rust side as `dai-event` (mirrors `DaiEvent` in the daemon). */
export type DaiEvent =
  | { type: "install_started"; id: string }
  | {
      type: "install_progress";
      id: string;
      stage: "download" | "index";
      bytes: number;
      total: number | null;
    }
  | { type: "install_finished"; id: string; error: string | null }
  | { type: "removed"; id: string }
  | { type: "snippets_changed" }
  | ({ type: "open" } & Page);

export const DAI_EVENT = "dai-event";

export const api = {
  daemonUrl: () => invoke<string>("daemon_url"),
  docsets: () => invoke<Docset[]>("docsets"),
  catalog: (refresh = false) => invoke<CatalogEntry[]>("catalog", { refresh }),
  outdated: (refresh = false) => invoke<Docset[]>("outdated", { refresh }),
  install: (id: string) => invoke<Docset>("install", { id }),
  remove: (id: string) => invoke<boolean>("remove", { id }),
  search: (query: string, docsets: string[], project?: string, limit = 50) =>
    invoke<Hit[]>("search", { query, docsets, project: project ?? null, limit }),
  project: (path: string) => invoke<ProjectReport>("project", { path }),
  initialOpen: () => invoke<Page | null>("initial_open"),
  generate: (source: GenerateSource, name?: string) =>
    invoke<Docset>("generate", { name: name || null, source }),
  context7Libraries: (name: string, query: string) =>
    invoke<Context7Library[]>("context7_libraries", { name, query }),
  context7Docs: (libraryId: string, query: string) =>
    invoke<string>("context7_docs", { libraryId, query }),
  snippets: (query: string, language?: string, tag?: string) =>
    invoke<Snippet[]>("snippets", { query, language, tag }),
  snippet: (id: string) => invoke<Snippet | null>("snippet", { id }),
  createSnippet: (input: SnippetInput) => invoke<Snippet>("create_snippet", { input }),
  updateSnippet: (id: string, input: SnippetInput) =>
    invoke<Snippet>("update_snippet", { id, input }),
  deleteSnippet: (id: string) => invoke<boolean>("delete_snippet", { id }),
};
