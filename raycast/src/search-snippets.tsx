import { useRef, useState } from "react";
import {
  Action,
  ActionPanel,
  Alert,
  confirmAlert,
  Icon,
  Keyboard,
  launchCommand,
  LaunchType,
  List,
  showToast,
  Toast,
} from "@raycast/api";
import { showFailureToast, usePromise } from "@raycast/utils";
import { api, DaemonDownError, type Snippet } from "./daemon";
import { snippetPath } from "./lib/discovery";
import { snippetDetail, snippetMarkdown } from "./lib/markdown";
import { NotRunningView, onError } from "./not-running";

export default function SearchSnippets() {
  const [query, setQuery] = useState("");

  const info = usePromise(() => api.info(), [], { onError });

  const abort = useRef<AbortController>(null);
  const snippets = usePromise((q: string) => api.snippets(q, abort.current?.signal), [query.trim()], {
    abortable: abort,
    onError,
  });

  const down = info.error instanceof DaemonDownError || snippets.error instanceof DaemonDownError;
  const reload = () => {
    info.revalidate();
    snippets.revalidate();
  };
  const items = snippets.data ?? [];

  return (
    <List
      isLoading={info.isLoading || snippets.isLoading}
      isShowingDetail={items.length > 0}
      filtering={false}
      throttle
      onSearchTextChange={setQuery}
      searchBarPlaceholder="Search snippets…"
    >
      {down ? (
        <NotRunningView onRetry={reload} />
      ) : items.length === 0 ? (
        snippets.isLoading ? (
          <List.EmptyView title="" />
        ) : (
          <List.EmptyView
            icon={Icon.Code}
            title={query.trim() ? "No matching snippets" : "No snippets yet"}
            actions={
              <ActionPanel>
                <Action
                  title="Save Snippet"
                  icon={Icon.Plus}
                  onAction={() => launchCommand({ name: "save-snippet", type: LaunchType.UserInitiated })}
                />
              </ActionPanel>
            }
          />
        )
      ) : (
        items.map((s) => (
          <List.Item
            key={s.id}
            title={s.title}
            detail={
              <List.Item.Detail
                markdown={snippetDetail(s)}
                metadata={
                  <List.Item.Detail.Metadata>
                    <List.Item.Detail.Metadata.Label title="Language" text={s.language || "—"} />
                    {s.tags.length > 0 ? (
                      <List.Item.Detail.Metadata.TagList title="Tags">
                        {s.tags.map((t) => (
                          <List.Item.Detail.Metadata.TagList.Item key={t} text={t} />
                        ))}
                      </List.Item.Detail.Metadata.TagList>
                    ) : null}
                    <List.Item.Detail.Metadata.Label title="Id" text={s.id} />
                  </List.Item.Detail.Metadata>
                }
              />
            }
            actions={<SnippetActions snippet={s} snippetsDir={info.data?.snippets_dir} onDeleted={snippets.revalidate} />}
          />
        ))
      )}
    </List>
  );
}

function SnippetActions({
  snippet,
  snippetsDir,
  onDeleted,
}: {
  snippet: Snippet;
  snippetsDir: string | undefined;
  onDeleted: () => void;
}) {
  const file = snippetsDir ? snippetPath(snippetsDir, snippet.id) : undefined;
  return (
    <ActionPanel>
      <Action.Paste title="Paste Code" content={snippet.code} />
      <Action.CopyToClipboard title="Copy Code" content={snippet.code} />
      <Action.CopyToClipboard
        title="Copy Snippet as Markdown"
        content={snippetMarkdown(snippet)}
        shortcut={Keyboard.Shortcut.Common.Copy}
      />
      {file ? (
        <>
          <Action.Open title="Open File in Editor" target={file} icon={Icon.Pencil} shortcut={Keyboard.Shortcut.Common.Open} />
          <Action.ShowInFinder path={file} shortcut={{ modifiers: ["cmd", "shift"], key: "f" }} />
        </>
      ) : null}
      <Action
        title="Delete Snippet"
        icon={Icon.Trash}
        style={Action.Style.Destructive}
        shortcut={Keyboard.Shortcut.Common.Remove}
        onAction={async () => {
          const ok = await confirmAlert({
            title: `Delete “${snippet.title}”?`,
            message: "This removes the snippet file.",
            primaryAction: { title: "Delete", style: Alert.ActionStyle.Destructive },
          });
          if (!ok) return;
          try {
            await api.deleteSnippet(snippet.id);
            await showToast({ style: Toast.Style.Success, title: "Snippet deleted" });
            onDeleted();
          } catch (e) {
            await showFailureToast(e, { title: "Couldn't delete snippet" });
          }
        }}
      />
    </ActionPanel>
  );
}
