import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { api, type GenerateSource } from "./api";
import styles from "./Docsets.module.css";

type Kind = GenerateSource["kind"];

const KINDS: Record<Kind, { label: string; placeholder: string }> = {
  llms: { label: "llms.txt", placeholder: "https://svelte.dev or …/llms.txt" },
  repo: { label: "Git repo", placeholder: "https://github.com/org/repo" },
  dir: { label: "Local folder", placeholder: "/path/to/markdown/docs" },
  context7: { label: "Context7", placeholder: "/org/project (library id)" },
};

/** Builds `md:` docsets for libraries without a (current) DevDocs/Dash docset. */
export function AddDocs() {
  const [kind, setKind] = useState<Kind>("llms");
  const [target, setTarget] = useState("");
  const [extra, setExtra] = useState("");
  const [name, setName] = useState("");

  const generate = useMutation({
    mutationFn: () => api.generate(toSource(kind, target.trim(), extra.trim()), name.trim()),
    onSuccess: () => {
      setTarget("");
      setExtra("");
      setName("");
    },
  });

  return (
    <form
      className={styles.addDocs}
      onSubmit={(e) => {
        e.preventDefault();
        generate.mutate();
      }}
    >
      <div className={styles.addRow}>
        <select value={kind} onChange={(e) => setKind(e.target.value as Kind)} aria-label="Source">
          {(Object.keys(KINDS) as Kind[]).map((k) => (
            <option key={k} value={k}>
              {KINDS[k].label}
            </option>
          ))}
        </select>
        <input
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          placeholder={KINDS[kind].placeholder}
          aria-label="Source location"
          required
        />
      </div>
      {kind === "repo" && (
        <input
          value={extra}
          onChange={(e) => setExtra(e.target.value)}
          placeholder="Branch or tag (optional)"
        />
      )}
      {kind === "context7" && (
        <input
          value={extra}
          onChange={(e) => setExtra(e.target.value)}
          placeholder="Topics, comma-separated (optional)"
        />
      )}
      <div className={styles.addRow}>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Name (optional)"
        />
        <button type="submit" disabled={generate.isPending || !target.trim()}>
          {generate.isPending ? "Generating…" : "Generate"}
        </button>
      </div>
      {generate.error && <p className={styles.error}>{String(generate.error)}</p>}
      {generate.data && <p className={styles.hint}>Added {generate.data.id}.</p>}
    </form>
  );
}

export function toSource(kind: Kind, target: string, extra: string): GenerateSource {
  switch (kind) {
    case "llms":
      return { kind, url: target };
    case "repo":
      return extra ? { kind, url: target, git_ref: extra } : { kind, url: target };
    case "dir":
      return { kind, path: target };
    case "context7":
      return {
        kind,
        library_id: target,
        topics: extra
          .split(",")
          .map((t) => t.trim())
          .filter(Boolean),
      };
  }
}
