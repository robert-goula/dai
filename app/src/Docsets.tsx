import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useDeferredValue, useState } from "react";
import { api, type CatalogEntry } from "./api";
import { AddDocs } from "./AddDocs";
import styles from "./Docsets.module.css";
import { forCatalogEntry, type InstallState } from "./useDaiEvents";

type Source = "" | CatalogEntry["source"];

export function Docsets({ installState }: { installState: InstallState }) {
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState("");
  const [source, setSource] = useState<Source>("");
  const deferredFilter = useDeferredValue(filter.trim().toLowerCase());

  const installed = useQuery({ queryKey: ["docsets"], queryFn: api.docsets });
  const outdated = useQuery({ queryKey: ["outdated"], queryFn: () => api.outdated() });
  const catalog = useQuery({
    queryKey: ["catalog"],
    queryFn: () => api.catalog(),
    staleTime: Infinity,
  });

  // Progress and errors arrive as daemon events, so these just fire the request.
  const install = useMutation({ mutationFn: api.install });
  const remove = useMutation({ mutationFn: api.remove });
  const checkUpdates = useMutation({
    mutationFn: async () => {
      queryClient.setQueryData(["outdated"], await api.outdated(true));
      queryClient.setQueryData(["catalog"], await api.catalog());
    },
  });

  const { installing, errors } = installState;
  const installedIds = new Set(installed.data?.map((d) => d.id));
  const outdatedIds = new Set(outdated.data?.map((d) => d.id));
  const available = (catalog.data ?? []).filter(
    (d) =>
      !installedIds.has(d.id) && (!source || d.source === source) && matches(d, deferredFilter),
  );

  return (
    <div className={styles.docsets}>
      <section className={styles.section}>
        <header className={styles.header}>
          <h2>Generate</h2>
        </header>
        <AddDocs />
        {[...installing]
          .filter(([id]) => id.startsWith("md:") && !installedIds.has(id))
          .map(([id, label]) => (
            <p key={id} className={styles.busy}>
              {id}: {label}
            </p>
          ))}
      </section>

      <section className={styles.section}>
        <header className={styles.header}>
          <h2>Installed</h2>
          <button onClick={() => checkUpdates.mutate()} disabled={checkUpdates.isPending}>
            {checkUpdates.isPending ? "Checking…" : "Check for updates"}
          </button>
          {outdatedIds.size > 0 && (
            <button onClick={() => outdatedIds.forEach((id) => install.mutate(id))}>
              Update all ({outdatedIds.size})
            </button>
          )}
        </header>
        {installed.data?.length === 0 && <p className={styles.hint}>Nothing installed yet.</p>}
        <ul className={styles.list}>
          {installed.data?.map((d) => (
            <li key={d.id} className={styles.row}>
              <div className={styles.info}>
                <span className={styles.name}>{d.name}</span>
                <span className={styles.meta}>
                  {d.id} · {d.version || d.release}
                  {outdatedIds.has(d.id) && <span className={styles.badge}>update</span>}
                </span>
                {errors.has(d.id) && <span className={styles.error}>{errors.get(d.id)}</span>}
              </div>
              {installing.has(d.id) ? (
                <span className={styles.busy}>{installing.get(d.id)}</span>
              ) : (
                <>
                  {outdatedIds.has(d.id) && (
                    <button onClick={() => install.mutate(d.id)}>Update</button>
                  )}
                  {d.id.startsWith("md:") && (
                    <button onClick={() => install.mutate(d.id)} title={`Re-run from ${d.release}`}>
                      Regenerate
                    </button>
                  )}
                  <button onClick={() => remove.mutate(d.id)}>Remove</button>
                </>
              )}
            </li>
          ))}
        </ul>
      </section>

      <section className={styles.section}>
        <header className={styles.header}>
          <h2>Available</h2>
          <select
            value={source}
            onChange={(e) => setSource(e.target.value as Source)}
            aria-label="Source"
          >
            <option value="">All sources</option>
            <option value="devdocs">DevDocs</option>
            <option value="dash">Dash</option>
          </select>
        </header>
        <input
          type="search"
          placeholder="Filter docsets"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          aria-label="Filter available docsets"
        />
        {catalog.error && <p className={styles.error}>{String(catalog.error)}</p>}
        <ul className={styles.list}>
          {available.map((d) => (
            <li key={d.id} className={styles.row}>
              <div className={styles.info}>
                <span className={styles.name}>{d.name}</span>
                <span className={styles.meta}>
                  <span className={styles.source}>{d.source}</span> {d.id}
                  {d.version && ` · ${d.version}`} · {formatSize(d.size)}
                </span>
                {forCatalogEntry(errors, d.id) && (
                  <span className={styles.error}>{forCatalogEntry(errors, d.id)}</span>
                )}
              </div>
              {forCatalogEntry(installing, d.id) ? (
                <span className={styles.busy}>{forCatalogEntry(installing, d.id)}</span>
              ) : (
                <InstallButton entry={d} onInstall={(id) => install.mutate(id)} />
              )}
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}

function matches(d: CatalogEntry, filter: string): boolean {
  return !filter || d.id.toLowerCase().includes(filter) || d.name.toLowerCase().includes(filter);
}

function formatSize(bytes: number): string {
  return bytes >= 1e6
    ? `${(bytes / 1e6).toFixed(1)} MB`
    : `${Math.max(1, Math.round(bytes / 1e3))} KB`;
}

/** Install button, with a version picker for docsets that offer older versions. */
function InstallButton({
  entry,
  onInstall,
}: {
  entry: CatalogEntry;
  onInstall: (id: string) => void;
}) {
  const [version, setVersion] = useState("");
  if (entry.versions.length === 0) {
    return <button onClick={() => onInstall(entry.id)}>Install</button>;
  }
  return (
    <span className={styles.install}>
      <select value={version} onChange={(e) => setVersion(e.target.value)} aria-label="Version">
        <option value="">{entry.version || "latest"}</option>
        {entry.versions.map((v) => (
          <option key={v} value={v}>
            {v}
          </option>
        ))}
      </select>
      <button onClick={() => onInstall(version ? `${entry.id}@${version}` : entry.id)}>
        Install
      </button>
    </span>
  );
}
