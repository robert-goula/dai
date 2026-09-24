import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { api } from "./api";
import styles from "./SnippetEditor.module.css";
import { emptyDraft, type SnippetDraft, toDraft, toInput } from "./snippetForm";

type Props = {
  /** Snippet to edit, or `null` for a new one. */
  id: string | null;
  onSaved: (id: string) => void;
  onDeleted: () => void;
};

export function SnippetEditor({ id, onSaved, onDeleted }: Props) {
  const queryClient = useQueryClient();
  const existing = useQuery({
    queryKey: ["snippet", id],
    queryFn: () => api.snippet(id!),
    enabled: id !== null,
  });
  const [draft, setDraft] = useState<SnippetDraft>(emptyDraft);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [copied, setCopied] = useState(false);

  // Load the snippet into the form when it's first fetched or its file actually
  // changes (`updated`), not on every refetch; other snippets changing triggers
  // refetches too, and those mustn't wipe unsaved edits.
  const loaded = existing.data;
  const loadedKey = id === null ? "new" : loaded ? `${loaded.id}@${loaded.updated}` : null;
  const [syncedKey, setSyncedKey] = useState<string | null>(null);
  if (loadedKey !== null && loadedKey !== syncedKey) {
    setSyncedKey(loadedKey);
    setDraft(loaded && id !== null ? toDraft(loaded) : emptyDraft);
    setConfirmDelete(false);
  }

  const refresh = () => queryClient.invalidateQueries({ queryKey: ["snippets"] });
  const save = useMutation({
    mutationFn: () =>
      id === null ? api.createSnippet(toInput(draft)) : api.updateSnippet(id, toInput(draft)),
    onSuccess: (s) => {
      queryClient.setQueryData(["snippet", s.id], s);
      void refresh();
      onSaved(s.id);
    },
  });
  const remove = useMutation({
    mutationFn: () => api.deleteSnippet(id!),
    onSuccess: () => {
      void refresh();
      onDeleted();
    },
  });

  const set = (field: keyof SnippetDraft) => (e: { target: { value: string } }) =>
    setDraft((d) => ({ ...d, [field]: e.target.value }));

  const copy = async () => {
    await navigator.clipboard.writeText(draft.code);
    setCopied(true);
    setTimeout(() => setCopied(false), 1200);
  };

  if (id !== null && existing.isSuccess && !existing.data) {
    return <div className={styles.empty}>This snippet no longer exists.</div>;
  }

  return (
    <form
      className={styles.editor}
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate();
      }}
    >
      <header className={styles.toolbar}>
        <input
          className={styles.title}
          placeholder="Title"
          value={draft.title}
          onChange={set("title")}
          aria-label="Title"
          required
        />
        <button type="button" onClick={() => void copy()} disabled={!draft.code}>
          {copied ? "Copied" : "Copy code"}
        </button>
        <button type="submit" disabled={save.isPending}>
          {save.isPending ? "Saving…" : "Save"}
        </button>
        {id !== null &&
          (confirmDelete ? (
            <button type="button" className={styles.danger} onClick={() => remove.mutate()}>
              Confirm delete
            </button>
          ) : (
            <button type="button" onClick={() => setConfirmDelete(true)}>
              Delete
            </button>
          ))}
      </header>

      {(save.error || remove.error) && (
        <p className={styles.error}>{String(save.error ?? remove.error)}</p>
      )}

      <div className={styles.fields}>
        <label>
          Language
          <input value={draft.language} onChange={set("language")} placeholder="e.g. rust" />
        </label>
        <label>
          Tags
          <input value={draft.tags} onChange={set("tags")} placeholder="react, hooks" />
        </label>
        <label className={styles.wide}>
          Description
          <input
            value={draft.description}
            onChange={set("description")}
            placeholder="What it does and when to use it"
          />
        </label>
      </div>

      <label className={styles.codeLabel}>
        Code
        <textarea
          className={styles.code}
          value={draft.code}
          onChange={set("code")}
          onKeyDown={insertTab}
          spellCheck={false}
          required
        />
      </label>
      <label className={styles.notesLabel}>
        Notes (markdown)
        <textarea className={styles.notes} value={draft.notes} onChange={set("notes")} />
      </label>
    </form>
  );
}

/** Tab inserts two spaces in the code field instead of moving focus. */
function insertTab(e: React.KeyboardEvent<HTMLTextAreaElement>) {
  if (e.key !== "Tab" || e.shiftKey) return;
  e.preventDefault();
  // execCommand keeps the edit on the undo stack; React sees the input event.
  document.execCommand("insertText", false, "  ");
}
