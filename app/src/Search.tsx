import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { type RefObject, useEffect, useRef, useState } from "react";
import { api, type Hit, type Page } from "./api";
import styles from "./Search.module.css";

type Props = {
  inputRef: RefObject<HTMLInputElement | null>;
  onOpen: (page: Page) => void;
};

export function Search({ inputRef, onOpen }: Props) {
  const [query, setQuery] = useState("");
  const [docset, setDocset] = useState("");
  const [active, setActive] = useState(0);
  const list = useRef<HTMLOListElement>(null);

  const docsets = useQuery({ queryKey: ["docsets"], queryFn: api.docsets });
  const q = query.trim();
  const results = useQuery({
    queryKey: ["search", q, docset],
    queryFn: () => api.search(q, docset ? [docset] : []),
    enabled: q.length > 0,
    placeholderData: keepPreviousData,
  });
  const hits = q ? (results.data ?? []) : [];

  useEffect(() => setActive(0), [q, docset]);
  useEffect(() => {
    list.current?.children[active]?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const open = (hit: Hit | undefined) => hit && onOpen({ docset: hit.docset, path: hit.path });

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, hits.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      open(hits[active]);
    }
  };

  const none = docsets.data?.length === 0;

  return (
    <div className={styles.search}>
      <div className={styles.controls}>
        <input
          ref={inputRef}
          autoFocus
          type="search"
          placeholder="Search docs  (⌘K)"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          aria-label="Search docs"
        />
        <select value={docset} onChange={(e) => setDocset(e.target.value)} aria-label="Docset">
          <option value="">All docsets</option>
          {docsets.data?.map((d) => (
            <option key={d.id} value={d.id}>
              {d.name} {d.version}
            </option>
          ))}
        </select>
      </div>

      {none && (
        <p className={styles.hint}>No docsets installed yet. Add some in the Docsets tab.</p>
      )}
      {results.error && <p className={styles.errorText}>{String(results.error)}</p>}
      {q && results.isSuccess && hits.length === 0 && <p className={styles.hint}>No results.</p>}

      <ol ref={list} className={styles.results}>
        {hits.map((h, i) => (
          <li key={`${h.docset}:${h.kind}:${h.path}:${h.heading}:${i}`}>
            <button
              className={styles.hit}
              aria-current={i === active}
              onClick={() => open(h)}
              onMouseMove={() => setActive(i)}
            >
              <span className={styles.name}>{h.name}</span>
              <span className={styles.meta}>
                <span className={styles.badge}>{h.docset}</span>
                {h.kind === "entry" ? h.entry_type : h.heading}
              </span>
              {h.snippet && <span className={styles.snippet}>{h.snippet}</span>}
            </button>
          </li>
        ))}
      </ol>
    </div>
  );
}
