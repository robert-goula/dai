import { useRef, useState } from "react";
import {
  Action,
  ActionPanel,
  Clipboard,
  Icon,
  Keyboard,
  List,
  open,
  showHUD,
  showToast,
  Toast,
} from "@raycast/api";
import { showFailureToast, usePromise } from "@raycast/utils";
import { api, DaemonDownError, type Hit } from "./daemon";
import { pageDetail } from "./lib/markdown";
import { NotRunningView, onError } from "./not-running";

const DETAIL_CHARS = 4000;

export default function SearchDocs() {
  const [query, setQuery] = useState("");
  const [docset, setDocset] = useState("");
  const [selected, setSelected] = useState<string | null>(null);

  const docsets = usePromise(() => api.docsets(), [], { onError });

  const searchAbort = useRef<AbortController>(null);
  const search = usePromise(
    (q: string, d: string) => api.search(q, d ? [d] : [], searchAbort.current?.signal),
    [query.trim(), docset],
    { execute: query.trim() !== "", abortable: searchAbort, onError },
  );
  const hits = query.trim() ? (search.data ?? []) : [];
  const hit = selected === null ? undefined : hits[Number(selected)];

  const docAbort = useRef<AbortController>(null);
  const doc = usePromise(
    (d: string, p: string) => api.doc(d, p, DETAIL_CHARS, docAbort.current?.signal),
    [hit?.docset ?? "", hit?.path ?? ""],
    { execute: hit !== undefined, abortable: docAbort, onError },
  );
  const page =
    hit && doc.data?.docset === hit.docset && doc.data.path === hit.path ? doc.data : undefined;

  // A cleared query stops `search` running, so its old error would otherwise stick around.
  const down =
    docsets.error instanceof DaemonDownError ||
    (query.trim() !== "" && search.error instanceof DaemonDownError);
  const retry = () => {
    docsets.revalidate();
    if (query.trim()) search.revalidate();
  };

  return (
    <List
      isLoading={docsets.isLoading || search.isLoading}
      isShowingDetail={hits.length > 0}
      filtering={false}
      throttle
      onSearchTextChange={setQuery}
      onSelectionChange={setSelected}
      searchBarPlaceholder="Search docs…"
      searchBarAccessory={
        <List.Dropdown tooltip="Docset" storeValue onChange={setDocset}>
          <List.Dropdown.Item title="All Docsets" value="" />
          {(docsets.data ?? []).map((d) => (
            <List.Dropdown.Item
              key={d.id}
              title={d.version ? `${d.name} ${d.version}` : d.name}
              value={d.id}
            />
          ))}
        </List.Dropdown>
      }
    >
      {down ? (
        <NotRunningView onRetry={retry} />
      ) : hits.length === 0 ? (
        <DocsEmptyView
          loading={docsets.isLoading || search.isLoading}
          noDocsets={docsets.data?.length === 0}
          query={query.trim()}
          filtered={docset !== ""}
        />
      ) : (
        hits.map((h, i) => (
          <List.Item
            key={i}
            id={String(i)}
            title={h.name}
            subtitle={h.heading || undefined}
            accessories={[{ tag: h.docset }]}
            detail={
              <List.Item.Detail
                isLoading={h === hit && !page}
                markdown={h === hit && page ? pageDetail(page) : undefined}
              />
            }
            actions={<HitActions hit={h} url={h === hit ? page?.url : undefined} />}
          />
        ))
      )}
    </List>
  );
}

function DocsEmptyView(props: {
  loading: boolean;
  noDocsets: boolean;
  query: string;
  filtered: boolean;
}) {
  // Avoid flashing "No results" while a search is in flight.
  if (props.loading) return <List.EmptyView title="" />;
  if (props.noDocsets) {
    return (
      <List.EmptyView
        icon={Icon.Book}
        title="No docsets installed"
        description="Install some from the DAI app."
        actions={
          <ActionPanel>
            <Action title="Open DAI" icon={Icon.AppWindow} onAction={() => open("dai://")} />
          </ActionPanel>
        }
      />
    );
  }
  if (!props.query)
    return <List.EmptyView icon={Icon.MagnifyingGlass} title="Search your installed docs" />;
  return (
    <List.EmptyView
      icon={Icon.MagnifyingGlass}
      title="No results"
      description={props.filtered ? "Try “All Docsets” in the dropdown." : undefined}
    />
  );
}

function HitActions({ hit, url }: { hit: Hit; url: string | undefined }) {
  return (
    <ActionPanel>
      <Action
        title="Open in DAI"
        icon={Icon.Book}
        onAction={async () => {
          try {
            await api.open(hit.docset, hit.path);
            await showHUD("Opened in DAI");
          } catch (e) {
            await showFailureToast(e, { title: "Couldn't open in DAI" });
          }
        }}
      />
      <Action
        title="Copy Page as Markdown"
        icon={Icon.CopyClipboard}
        onAction={async () => {
          const toast = await showToast({ style: Toast.Style.Animated, title: "Fetching page…" });
          try {
            const full = await api.doc(hit.docset, hit.path, Number.MAX_SAFE_INTEGER);
            await Clipboard.copy(full.markdown);
            toast.style = Toast.Style.Success;
            toast.title = "Copied page";
          } catch (e) {
            await toast.hide();
            await showFailureToast(e, { title: "Couldn't copy page" });
          }
        }}
      />
      {url ? (
        <>
          <Action.OpenInBrowser
            title="Open Upstream URL"
            url={url}
            shortcut={Keyboard.Shortcut.Common.Open}
          />
          <Action.CopyToClipboard
            title="Copy Upstream URL"
            content={url}
            shortcut={Keyboard.Shortcut.Common.Copy}
          />
        </>
      ) : null}
      <Action.CopyToClipboard
        title="Copy Docset/Path"
        content={`${hit.docset}/${hit.path}`}
        shortcut={Keyboard.Shortcut.Common.CopyPath}
      />
    </ActionPanel>
  );
}
