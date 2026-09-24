import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useDeferredValue, useState } from "react";
import { api, type CatalogDoc } from "./api";
import styles from "./Docsets.module.css";
import type { InstallState } from "./useDaiEvents";

export function Docsets({ installState }: { installState: InstallState }) {
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState("");
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
    (d) => !installedIds.has(d.slug) && matches(d, deferredFilter),
  );

  return (
    <div className={styles.docsets}>
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
                <span className={styles.busy}>Updating…</span>
              ) : (
                <>
                  {outdatedIds.has(d.id) && (
                    <button onClick={() => install.mutate(d.id)}>Update</button>
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
          <span className={styles.meta}>DevDocs</span>
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
            <li key={d.slug} className={styles.row}>
              <div className={styles.info}>
                <span className={styles.name}>{d.name}</span>
                <span className={styles.meta}>
                  {d.slug} · {d.release || d.version} · {formatSize(d.db_size)}
                </span>
                {errors.has(d.slug) && <span className={styles.error}>{errors.get(d.slug)}</span>}
              </div>
              {installing.has(d.slug) ? (
                <span className={styles.busy}>Installing…</span>
              ) : (
                <button onClick={() => install.mutate(d.slug)}>Install</button>
              )}
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}

function matches(d: CatalogDoc, filter: string): boolean {
  return !filter || d.slug.includes(filter) || d.name.toLowerCase().includes(filter);
}

function formatSize(bytes: number): string {
  return bytes >= 1e6
    ? `${(bytes / 1e6).toFixed(1)} MB`
    : `${Math.max(1, Math.round(bytes / 1e3))} KB`;
}
