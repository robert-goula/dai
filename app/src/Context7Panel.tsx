import { openUrl } from "@tauri-apps/plugin-opener";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { api } from "./api";
import styles from "./Context7Panel.module.css";

/** Looks up docs on Context7 when local docsets don't have them. */
export function Context7Panel({ initialQuery }: { initialQuery: string }) {
  const [name, setName] = useState(initialQuery.split(/\s+/)[0] ?? "");
  const [query, setQuery] = useState(initialQuery);
  const [libraryId, setLibraryId] = useState<string | null>(null);
  const [asked, setAsked] = useState<{ libraryId: string; query: string } | null>(null);

  const libraries = useQuery({
    queryKey: ["context7", "libraries", name.trim(), query.trim()],
    queryFn: () => api.context7Libraries(name.trim(), query.trim()),
    enabled: name.trim().length > 0,
    staleTime: 5 * 60_000,
  });
  const docs = useQuery({
    queryKey: ["context7", "docs", asked],
    queryFn: () => api.context7Docs(asked!.libraryId, asked!.query),
    enabled: asked !== null,
    staleTime: Infinity,
  });
  const save = useMutation({
    mutationFn: () =>
      api.generate({ kind: "context7", library_id: asked!.libraryId, topics: [asked!.query] }),
  });

  const selected = libraryId ?? libraries.data?.[0]?.id ?? null;

  return (
    <div className={styles.panel}>
      <form
        className={styles.controls}
        onSubmit={(e) => {
          e.preventDefault();
          if (selected && query.trim()) setAsked({ libraryId: selected, query: query.trim() });
        }}
      >
        <input
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            setLibraryId(null);
          }}
          placeholder="Library name"
          aria-label="Library name"
        />
        <select
          value={selected ?? ""}
          onChange={(e) => setLibraryId(e.target.value)}
          aria-label="Library"
          disabled={!libraries.data?.length}
        >
          {libraries.isFetching && !libraries.data && <option>Searching…</option>}
          {libraries.data?.length === 0 && <option>No libraries found</option>}
          {libraries.data?.map((l) => (
            <option key={l.id} value={l.id}>
              {l.title} — {l.id} ({Math.round(l.totalTokens / 1000)}k tokens)
            </option>
          ))}
        </select>
        <input
          className={styles.query}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="What do you want to know?"
          aria-label="Question"
        />
        <button type="submit" disabled={!selected || !query.trim()}>
          Ask Context7
        </button>
      </form>

      {libraries.error && <p className={styles.error}>{String(libraries.error)}</p>}

      <div className={styles.results}>
        {docs.isFetching && <p className={styles.hint}>Fetching from Context7…</p>}
        {docs.error && <p className={styles.error}>{String(docs.error)}</p>}
        {docs.data && asked && (
          <>
            <header className={styles.resultHeader}>
              <span>
                {asked.libraryId} · “{asked.query}”
              </span>
              <button onClick={() => save.mutate()} disabled={save.isPending || save.isSuccess}>
                {save.isSuccess
                  ? `Saved as ${save.data.id}`
                  : save.isPending
                    ? "Saving…"
                    : "Save as docset"}
              </button>
            </header>
            {save.error && <p className={styles.error}>{String(save.error)}</p>}
            <article className={styles.markdown}>
              <Markdown
                remarkPlugins={[remarkGfm]}
                components={{
                  a: ({ href, children }) => (
                    <a
                      href={href}
                      onClick={(e) => {
                        e.preventDefault();
                        if (href) void openUrl(href);
                      }}
                    >
                      {children}
                    </a>
                  ),
                }}
              >
                {docs.data}
              </Markdown>
            </article>
          </>
        )}
        {!asked && (
          <p className={styles.hint}>
            Pick a library and ask a question. Results come from context7.com; save them to keep
            them offline and searchable (they're also available to your agents).
          </p>
        )}
      </div>
    </div>
  );
}
