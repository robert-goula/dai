import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useRef, useState } from "react";
import type { Page } from "./api";
import { contentUrl } from "./content";
import styles from "./Viewer.module.css";

type Props = {
  /** Daemon base URL; undefined while connecting. */
  base: string | undefined;
  page: Page | null;
  /** Changes on every open, even of the same page. */
  openCount: number;
};

/** Messages posted by the viewer shell inside the iframe (see daemon `viewer.rs`). */
type ShellMessage =
  | { type: "dai:page"; docset: string; path: string; title: string }
  | { type: "dai:external"; url: string };

export function Viewer({ base, page, openCount }: Props) {
  const frame = useRef<HTMLIFrameElement>(null);
  // What the iframe is actually showing; differs from `page` after in-page link clicks.
  const [shown, setShown] = useState<{ docset: string; path: string; title: string } | null>(null);

  useEffect(() => {
    if (!base) return;
    const origin = new URL(base).origin;
    const onMessage = (e: MessageEvent<ShellMessage>) => {
      if (e.origin !== origin) return;
      if (e.data.type === "dai:page") {
        setShown({ docset: e.data.docset, path: e.data.path, title: e.data.title });
      } else if (e.data.type === "dai:external") {
        void openUrl(e.data.url);
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [base]);

  // Set imperatively: after in-page link clicks the iframe's location differs
  // from `page`, and React won't re-apply an unchanged `src` attribute.
  useEffect(() => {
    if (base && page && frame.current && openCount > 0) {
      frame.current.src = contentUrl(base, page.docset, page.path);
    }
  }, [base, page, openCount]);

  if (!base) return <div className={styles.empty}>Connecting to the DAI daemon…</div>;
  if (!page) {
    return (
      <div className={styles.empty}>
        <p>Search for a symbol or topic to get started.</p>
        <p className={styles.keys}>⌘K search · ↑↓ select · ↩ open</p>
      </div>
    );
  }

  return (
    <div className={styles.viewer}>
      <header className={styles.toolbar}>
        {/* The iframe's navigations join this window's history. */}
        <button onClick={() => history.back()} aria-label="Back">
          ←
        </button>
        <button onClick={() => history.forward()} aria-label="Forward">
          →
        </button>
        <span className={styles.location}>
          <span className={styles.docset}>{shown?.docset ?? page.docset}</span>
          <span className={styles.path}>{shown?.path ?? page.path}</span>
        </span>
      </header>
      <iframe
        className={styles.frame}
        title="Documentation"
        ref={frame}
        sandbox="allow-scripts allow-same-origin"
      />
    </div>
  );
}
