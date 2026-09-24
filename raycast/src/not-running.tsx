import { Action, ActionPanel, Icon, List, open, showToast, Toast } from "@raycast/api";

export function NotRunningView({ onRetry }: { onRetry: () => void }) {
  return (
    <List.EmptyView
      icon={Icon.Plug}
      title="DAI service isn't running"
      description="Start DAI, then retry. To keep it running, turn on “Start DAI service at login” in the app."
      actions={
        <ActionPanel>
          <Action
            title="Start DAI"
            icon={Icon.Play}
            onAction={async () => {
              try {
                await open("dai://");
              } catch (e) {
                await showToast({ style: Toast.Style.Failure, title: "Couldn't launch DAI", message: String(e) });
              }
            }}
          />
          <Action title="Retry" icon={Icon.ArrowClockwise} onAction={onRetry} />
        </ActionPanel>
      }
    />
  );
}
