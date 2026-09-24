import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import styles from "./App.module.css";

/** Toggles running the DAI background service (not the window) at login. */
export function StartAtLogin() {
  const queryClient = useQueryClient();
  const enabled = useQuery({ queryKey: ["autostart"], queryFn: isEnabled });
  const toggle = useMutation({
    mutationFn: (on: boolean) => (on ? enable() : disable()),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["autostart"] }),
  });

  return (
    <label
      className={styles.autostart}
      title="Keeps the DAI service running for your agents after you log in. The window doesn't open."
    >
      <input
        type="checkbox"
        checked={enabled.data ?? false}
        disabled={enabled.isPending || toggle.isPending}
        onChange={(e) => toggle.mutate(e.target.checked)}
      />
      Start DAI service at login
      {toggle.error && <span className={styles.footerError}> ({String(toggle.error)})</span>}
    </label>
  );
}
