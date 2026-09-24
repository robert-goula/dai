import { listen } from "@tauri-apps/api/event";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { DAI_EVENT, type DaiEvent, type Page } from "./api";

export type InstallState = {
  /** Docsets being installed or updated (from any client: app, CLI) -> status label. */
  installing: ReadonlyMap<string, string>;
  errors: ReadonlyMap<string, string>;
};

/** Tracks daemon events: keeps docset queries fresh and forwards `open` requests. */
export function useDaiEvents(onOpen: (page: Page) => void): InstallState {
  const queryClient = useQueryClient();
  const [state, setState] = useState<InstallState>({ installing: new Map(), errors: new Map() });

  useEffect(() => {
    const unlisten = listen<DaiEvent>(DAI_EVENT, ({ payload: e }) => {
      switch (e.type) {
        case "open":
          onOpen({ docset: e.docset, path: e.path });
          break;
        case "install_started":
          setState((s) => ({
            installing: new Map(s.installing).set(e.id, "Starting…"),
            errors: withoutKey(s.errors, e.id),
          }));
          break;
        case "install_progress":
          setState((s) => ({
            ...s,
            installing: new Map(s.installing).set(e.id, progressLabel(e)),
          }));
          break;
        case "install_finished":
          setState((s) => {
            const installing = new Map(s.installing);
            installing.delete(e.id);
            const errors = e.error ? new Map(s.errors).set(e.id, e.error) : s.errors;
            return { installing, errors };
          });
          invalidate();
          break;
        case "removed":
          invalidate();
          break;
      }
    });
    function invalidate() {
      for (const key of ["docsets", "outdated", "search"]) {
        void queryClient.invalidateQueries({ queryKey: [key] });
      }
    }
    return () => void unlisten.then((f) => f());
  }, [onOpen, queryClient]);

  return state;
}

export function progressLabel(e: {
  stage: "download" | "index";
  bytes: number;
  total: number | null;
}): string {
  if (e.stage === "index") return "Indexing…";
  if (e.total) return `Downloading ${Math.floor((e.bytes / e.total) * 100)}%`;
  return `Downloading ${(e.bytes / 1e6).toFixed(0)} MB`;
}

function withoutKey<K, V>(map: ReadonlyMap<K, V>, key: K): ReadonlyMap<K, V> {
  if (!map.has(key)) return map;
  const next = new Map(map);
  next.delete(key);
  return next;
}
