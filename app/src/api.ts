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
  | ({ type: "open" } & Page);

export const DAI_EVENT = "dai-event";

export const api = {
  daemonUrl: () => invoke<string>("daemon_url"),
  docsets: () => invoke<Docset[]>("docsets"),
  catalog: (refresh = false) => invoke<CatalogEntry[]>("catalog", { refresh }),
  outdated: (refresh = false) => invoke<Docset[]>("outdated", { refresh }),
  install: (id: string) => invoke<Docset>("install", { id }),
  remove: (id: string) => invoke<boolean>("remove", { id }),
  search: (query: string, docsets: string[], limit = 50) =>
    invoke<Hit[]>("search", { query, docsets, limit }),
  initialOpen: () => invoke<Page | null>("initial_open"),
};
