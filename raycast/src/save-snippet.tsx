import { useState } from "react";
import {
  Action,
  ActionPanel,
  Clipboard,
  Form,
  getSelectedText,
  open,
  popToRoot,
  showHUD,
  showToast,
  Toast,
} from "@raycast/api";
import { FormValidation, showFailureToast, usePromise, useForm } from "@raycast/utils";
import { api, DaemonDownError } from "./daemon";
import { languageOptions, parseTags } from "./lib/snippet-form";

type Values = { title: string; language: string; tags: string; description: string; code: string };

/** The selected text in the frontmost app, else the clipboard. */
async function initialCode(): Promise<string> {
  try {
    const selected = await getSelectedText();
    if (selected.trim()) return selected;
  } catch {
    // Nothing selected, or the app doesn't expose a selection.
  }
  return (await Clipboard.readText()) ?? "";
}

export default function SaveSnippet() {
  const [typedLanguage, setTypedLanguage] = useState("");

  const { handleSubmit, itemProps, setValue, values } = useForm<Values>({
    initialValues: { language: "" },
    validation: { title: FormValidation.Required, code: FormValidation.Required },
    async onSubmit(v) {
      const toast = await showToast({ style: Toast.Style.Animated, title: "Saving snippet…" });
      try {
        const saved = await api.createSnippet({
          title: v.title.trim(),
          language: v.language,
          tags: parseTags(v.tags),
          description: v.description.trim(),
          code: v.code,
          notes: "",
        });
        await toast.hide();
        await showHUD(`Saved “${saved.title}”`);
        await popToRoot();
      } catch (e) {
        await toast.hide();
        if (e instanceof DaemonDownError) {
          await showToast({
            style: Toast.Style.Failure,
            title: e.message,
            primaryAction: { title: "Start DAI", onAction: () => open("dai://") },
          });
        } else {
          await showFailureToast(e, { title: "Couldn't save snippet" });
        }
      }
    },
  });

  const code = usePromise(initialCode, [], {
    onData: (text) => {
      if (!values.code) setValue("code", text);
    },
  });

  // Seeds the language list; if the daemon is down we find out on submit.
  const existing = usePromise(() => api.snippets(""), [], { onError: () => {} });
  const languages = languageOptions([...(existing.data ?? []).map((s) => s.language), values.language], typedLanguage);

  return (
    <Form
      isLoading={code.isLoading || existing.isLoading}
      actions={
        <ActionPanel>
          <Action.SubmitForm title="Save Snippet" onSubmit={handleSubmit} />
        </ActionPanel>
      }
    >
      <Form.TextField title="Title" placeholder="Debounce hook" {...itemProps.title} />
      <Form.Dropdown
        title="Language"
        filtering
        onSearchTextChange={setTypedLanguage}
        info="Pick one, or type a new one."
        {...itemProps.language}
      >
        <Form.Dropdown.Item value="" title="None" />
        {languages.map((l) => (
          <Form.Dropdown.Item key={l} value={l} title={l} />
        ))}
      </Form.Dropdown>
      <Form.TextField title="Tags" placeholder="react, hooks" info="Comma-separated." {...itemProps.tags} />
      <Form.TextField title="Description" placeholder="What it's for" {...itemProps.description} />
      <Form.TextArea title="Code" enableMarkdown={false} {...itemProps.code} />
    </Form>
  );
}
