// Unsaved edits in the snippet editor tabs (#10). Unlike a host's, a
// snippet's edits live in its editor, which keeps them until saved, so this
// registry only lets the window ask about them before a tab closes, the
// window quits or another workspace opens, as it does for hosts.

import { tick } from "svelte";
import { question } from "./hostDrafts.svelte";

/** What a snippet question calls a snippet that has no name yet. */
export const NEW_SNIPPET = "a new snippet";

export interface SnippetEntry {
  /** The snippet's name as being edited, or `NEW_SNIPPET`. */
  name: string;
  /** Whether it has edits that are not saved. */
  changed: boolean;
  /** Saves it; resolves to whether it did (why not: `error`). */
  save: () => Promise<boolean>;
  /** Why the last save failed, if it did. */
  error: () => string | null;
}

/** The open snippet editors by their tab id. */
const entries = $state<Record<number, SnippetEntry>>({});

/** Lists the editor in `tab`, or updates its entry. */
export function registerSnippet(tab: number, entry: SnippetEntry) {
  entries[tab] = entry;
}

/** Removes the entry of `tab` (its editor is gone). */
export function unregisterSnippet(tab: number) {
  delete entries[tab];
}

/** The tabs of snippet editors with unsaved changes. */
export function snippetsWithUnsaved(): number[] {
  return Object.keys(entries)
    .map(Number)
    .filter((tab) => entries[tab].changed);
}

/**
 * Before something that would drop the unsaved changes of the snippet
 * editors in `tabs` (closing a tab, quitting, another workspace): asks
 * whether to save them, discard them or stay. Resolves to whether to go
 * on. Discarding drops nothing here: the editors go with their tabs.
 * Asks nothing when nothing is unsaved.
 */
export async function settleUnsavedSnippets(
  tabs: number[],
  doing: string,
): Promise<boolean> {
  let pending = tabs.filter((tab) => entries[tab]?.changed);
  if (pending.length === 0) return true;
  // One question at a time: an earlier one still open is cancelled.
  question.current?.answer("cancel");
  let error: string | null = null;
  for (;;) {
    const snippets = pending.map((tab) => entries[tab].name);
    let asked: ((choice: "save" | "discard" | "cancel") => void) | null = null;
    const choice = await new Promise<"save" | "discard" | "cancel">(
      (answer) => {
        asked = answer;
        question.current = { hosts: [], snippets, doing, error, answer };
      },
    );
    // A later question may already have taken the slot. Compared by its
    // answer: the state holds a proxy of the question, not the object.
    if (question.current?.answer === asked) question.current = null;
    if (choice === "cancel") return false;
    if (choice === "discard") return true;
    let failed: string | null = null;
    for (const tab of pending) {
      const entry = entries[tab];
      if (entry?.changed && !(await entry.save())) {
        failed ??= entry.error() ?? "it could not be saved";
      }
    }
    // The entries follow what was saved a moment later.
    await tick();
    pending = pending.filter((tab) => entries[tab]?.changed);
    if (pending.length === 0) return true;
    // Asked again, with why: the edits stay until saved or discarded.
    error = failed ?? entries[pending[0]].error();
  }
}
