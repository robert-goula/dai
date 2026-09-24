import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { type RefObject, useEffect, useRef, useState } from "react";
import { api, type Hit, type Page } from "./api";
import { folderName, projectStatus } from "./project";
import styles from "./Search.module.css";

type Props = {
  inputRef: RefObject<HTMLInputElement | null>;
  onOpen: (page: Page) => void;
  /** Look the query up on Context7 instead. */
  onAskContext7: (query: string) => void;
};

export function Search({ inputRef, onOpen, onAskContext7 }: Props) {
  const [query, setQuery] = useState("");
  const [docset, setDocset] = useState("");
  const [active, setActive] = useState(0);
  const [project, setProject] = useState<string | null>(loadProject);
  const list = useRef<HTMLOListElement>(null);

  const docsets = useQuery({ queryKey: ["docsets"], queryFn: api.docsets });
  const q = query.trim();
  const report = useQuery({
    queryKey: ["project", project],
    queryFn: () => api.project(project!),
    enabled: project !== null,
  });
  const results = useQuery({
    queryKey: ["search", q, docset, project],
    queryFn: () => api.search(q, docset ? [docset] : [], project ?? undefined),
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
  const status = report.data ? projectStatus(report.data) : null;

  const chooseProject = async () => {
    const dir = await openDialog({ directory: true, title: "Choose a project folder" });
    if (typeof dir === "string") updateProject(dir);
  };
  const updateProject = (dir: string | null) => {
    setProject(dir);
    saveProject(dir);
  };

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

      <div className={styles.project}>
        {project ? (
          <>
            <span className={styles.projectName} title={status?.details.join("\n")}>
              Versions from <strong>{folderName(project)}</strong>
              {status && ` · ${status.summary}`}
              {report.error && " · no manifest found"}
            </span>
            <button onClick={() => updateProject(null)} aria-label="Stop using project versions">
              ✕
            </button>
          </>
        ) : (
          <button className={styles.projectPick} onClick={() => void chooseProject()}>
            Match a project's versions…
          </button>
        )}
      </div>

      {none && (
        <p className={styles.hint}>No docsets installed yet. Add some in the Docsets tab.</p>
      )}
      {results.error && <p className={styles.errorText}>{String(results.error)}</p>}
      {q && results.isSuccess && hits.length === 0 && <p className={styles.hint}>No results.</p>}
      {q && results.isSuccess && (
        <button className={styles.context7} onClick={() => onAskContext7(q)}>
          Not finding it? Ask Context7 about “{q}”
        </button>
      )}

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

// The chosen project is a per-machine convenience, so browser storage is
// fine; it may be unavailable, hence the try/catch.
const PROJECT_KEY = "dai.project";

function loadProject(): string | null {
  try {
    return localStorage.getItem(PROJECT_KEY);
  } catch {
    return null;
  }
}

function saveProject(dir: string | null) {
  try {
    if (dir) localStorage.setItem(PROJECT_KEY, dir);
    else localStorage.removeItem(PROJECT_KEY);
  } catch {
    // Not persisted; still used for this session.
  }
}
