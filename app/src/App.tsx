import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Page } from "./api";
import styles from "./App.module.css";
import { Docsets } from "./Docsets";
import { Search } from "./Search";
import { SnippetEditor } from "./SnippetEditor";
import { Snippets } from "./Snippets";
import { useDaiEvents } from "./useDaiEvents";
import { Viewer } from "./Viewer";

type Tab = "search" | "snippets" | "docsets";
const TABS: Record<Tab, string> = { search: "Search", snippets: "Snippets", docsets: "Docsets" };

/** What the main pane shows: the doc viewer or the snippet editor (`id: null` = new). */
type MainView = { kind: "doc" } | { kind: "snippet"; id: string | null };

export function App() {
  const [tab, setTab] = useState<Tab>("search");
  // `n` bumps on every open so re-opening the same page still navigates.
  const [opened, setOpened] = useState<{ page: Page; n: number } | null>(null);
  const [main, setMain] = useState<MainView>({ kind: "doc" });
  const searchInput = useRef<HTMLInputElement>(null);
  const base = useQuery({ queryKey: ["daemonUrl"], queryFn: api.daemonUrl, staleTime: Infinity });

  const open = useCallback((page: Page) => {
    setOpened((o) => ({ page, n: (o?.n ?? 0) + 1 }));
    setMain({ kind: "doc" });
  }, []);
  const editSnippet = useCallback((id: string | null) => setMain({ kind: "snippet", id }), []);
  const installState = useDaiEvents(open);

  // Launched from a `dai://open` link.
  useEffect(() => {
    void api.initialOpen().then((p) => p && open(p));
  }, [open]);

  // Cmd/Ctrl+K jumps to search.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setTab("search");
        requestAnimationFrame(() => searchInput.current?.select());
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className={styles.app}>
      <aside className={styles.sidebar}>
        <nav className={styles.tabs} role="tablist">
          {(Object.keys(TABS) as Tab[]).map((t) => (
            <button
              key={t}
              role="tab"
              aria-selected={tab === t}
              className={styles.tab}
              onClick={() => setTab(t)}
            >
              {TABS[t]}
            </button>
          ))}
        </nav>
        {tab === "search" && <Search inputRef={searchInput} onOpen={open} />}
        {tab === "snippets" && (
          <Snippets selected={main.kind === "snippet" ? main.id : null} onSelect={editSnippet} />
        )}
        {tab === "docsets" && <Docsets installState={installState} />}
      </aside>
      <main className={styles.main}>
        {base.error ? (
          <div className={styles.error}>Could not reach the DAI daemon: {String(base.error)}</div>
        ) : (
          // Kept mounted while editing snippets so the iframe keeps its history.
          <div className={styles.pane} hidden={main.kind !== "doc"}>
            <Viewer base={base.data} page={opened?.page ?? null} openCount={opened?.n ?? 0} />
          </div>
        )}
        {main.kind === "snippet" && (
          <SnippetEditor
            key={main.id ?? "new"}
            id={main.id}
            onSaved={editSnippet}
            onDeleted={() => setMain({ kind: "doc" })}
          />
        )}
      </main>
    </div>
  );
}
