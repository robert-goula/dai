import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api } from "./api";
import styles from "./Snippets.module.css";

type Props = {
  selected: string | null;
  /** A snippet id, or `null` to start a new one. */
  onSelect: (id: string | null) => void;
};

export function Snippets({ selected, onSelect }: Props) {
  const [query, setQuery] = useState("");
  const [language, setLanguage] = useState("");

  const all = useQuery({ queryKey: ["snippets", ""], queryFn: () => api.snippets("") });
  const q = query.trim();
  const results = useQuery({
    queryKey: ["snippets", q, language],
    queryFn: () => api.snippets(q, language || undefined),
    placeholderData: keepPreviousData,
  });
  const languages = [...new Set(all.data?.map((s) => s.language).filter(Boolean))].sort();

  return (
    <div className={styles.snippets}>
      <div className={styles.controls}>
        <div className={styles.row}>
          <input
            type="search"
            placeholder="Search snippets"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            aria-label="Search snippets"
          />
          <button onClick={() => onSelect(null)}>New</button>
        </div>
        <select
          value={language}
          onChange={(e) => setLanguage(e.target.value)}
          aria-label="Language"
        >
          <option value="">All languages</option>
          {languages.map((l) => (
            <option key={l} value={l}>
              {l}
            </option>
          ))}
        </select>
      </div>

      {results.error && <p className={styles.error}>{String(results.error)}</p>}
      {all.data?.length === 0 && (
        <p className={styles.hint}>
          No snippets yet. Create one here, ask an agent to save one, or drop <code>.md</code> files
          into the snippets folder.
        </p>
      )}

      <ul className={styles.list}>
        {results.data?.map((s) => (
          <li key={s.id}>
            <button
              className={styles.item}
              aria-current={s.id === selected}
              onClick={() => onSelect(s.id)}
            >
              <span className={styles.title}>{s.title}</span>
              <span className={styles.meta}>
                {s.language && <span className={styles.badge}>{s.language}</span>}
                {s.tags.map((t) => `#${t}`).join(" ")}
              </span>
              {s.description && <span className={styles.description}>{s.description}</span>}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
