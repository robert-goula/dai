import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

export type Theme = "system" | "light" | "dark";
export type Scheme = "light" | "dark";

const KEY = "dai.theme";

// Per-machine preference, so browser storage is fine; it may be unavailable.
export function loadTheme(): Theme {
  try {
    const t = localStorage.getItem(KEY);
    return t === "light" || t === "dark" ? t : "system";
  } catch {
    return "system";
  }
}

function saveTheme(t: Theme) {
  try {
    if (t === "system") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, t);
  } catch {
    // Not persisted; still applied for this session.
  }
}

/** Sets the page's `color-scheme` explicitly (all colors use `light-dark()`). */
export function applyScheme(scheme: Scheme) {
  document.documentElement.style.colorScheme = scheme;
}

/** Forces the native window appearance, or (`system`) lets it follow the OS. */
function setNativeTheme(t: Theme): Promise<void> {
  return getCurrentWindow()
    .setTheme(t === "system" ? null : t)
    .catch(() => {});
}

/** Best guess before React mounts, so a dark choice doesn't flash light. */
export function applyInitialTheme() {
  const t = loadTheme();
  applyScheme(resolveScheme(t, window.matchMedia("(prefers-color-scheme: dark)").matches));
}

export function resolveScheme(t: Theme, systemDark: boolean): Scheme {
  return t === "system" ? (systemDark ? "dark" : "light") : t;
}

/**
 * The chosen theme and the scheme it resolves to. "System" asks Tauri for the
 * OS appearance: inside the app's web view, `prefers-color-scheme` doesn't
 * reliably follow macOS, so the media query is only a fallback (e.g. in a
 * plain browser).
 */
export function useTheme() {
  const [theme, setTheme] = useState<Theme>(loadTheme);
  const [systemDark, setSystemDark] = useState(
    () => window.matchMedia("(prefers-color-scheme: dark)").matches,
  );

  const readOsTheme = async (t: Theme) => {
    await setNativeTheme(t);
    if (t !== "system") return;
    const os = await getCurrentWindow()
      .theme()
      .catch(() => null);
    if (os) setSystemDark(os === "dark");
  };

  useEffect(() => {
    void readOsTheme(loadTheme());
    // With no forced theme the window follows the OS, so this tracks OS changes.
    const unlisten = getCurrentWindow()
      .onThemeChanged(({ payload }) => setSystemDark(payload === "dark"))
      .catch(() => null);
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    mq.addEventListener("change", onChange);
    return () => {
      mq.removeEventListener("change", onChange);
      void unlisten.then((f) => f?.());
    };
  }, []);

  const scheme = resolveScheme(theme, systemDark);
  useEffect(() => applyScheme(scheme), [scheme]);

  const choose = (t: Theme) => {
    setTheme(t);
    saveTheme(t);
    void readOsTheme(t);
  };

  return { theme, scheme, choose };
}
