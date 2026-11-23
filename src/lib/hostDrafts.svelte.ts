// Unsaved edits to hosts in the host editor tab (#10). A host's files go
// into git, so the editor collects edits into a draft and writes them on
// Save, rather than writing the file on every change; Revert drops them.
//
// A draft keeps the host as it was when editing began (`base`) beside the
// edited copy. Save writes only the fields that differ from `base`, over
// the host as it is saved by then, so a change that arrived meanwhile from
// outside the app (a git pull) to another field is kept. The name is not
// a field of the file but its file name, so it is renamed after the rest
// is saved.

import { inventory, renameHostOrSay, saveHostOrSay } from "./inventory.svelte";
import { findHost, type Host, type Settings } from "./workspace";

interface Draft {
  /** The host as saved when the first edit was made. */
  base: Host;
  /** The host with the edits. */
  edited: Host;
  /** Why the last Save failed, if it did. */
  error: string | null;
}

/** The drafts by host id. Only the host editor makes them. */
const drafts = $state<Record<string, Draft>>({});

/** The host the editor tab that follows the selection shows, so that the
 * window can ask about its edits when that tab closes. */
export const following = $state<{ host: string | null }>({ host: null });

/** Empty text and null mean the same to a host's file. */
function same(a: unknown, b: unknown): boolean {
  const plain = (value: unknown) => (value === "" ? null : value);
  return JSON.stringify(plain(a)) === JSON.stringify(plain(b));
}

type HostField = Exclude<keyof Host, "id" | "settings">;
const HOST_FIELDS = (h: Host) =>
  (Object.keys(h) as (keyof Host)[]).filter(
    (key): key is HostField => key !== "id" && key !== "settings",
  );

/** The fields the draft changes: host fields by name, settings as
 * `settings.<key>`. */
function changedFields(draft: Draft): string[] {
  const { base, edited } = draft;
  const fields: string[] = HOST_FIELDS(edited).filter(
    (key) => !same(base[key], edited[key]),
  );
  for (const key of Object.keys(edited.settings) as (keyof Settings)[]) {
    if (!same(base.settings[key], edited.settings[key]))
      fields.push(`settings.${key}`);
  }
  return fields;
}

/** The host `id` as the editor shows it: its draft, else as saved. */
export function shown(host: Host): Host {
  return drafts[host.id]?.edited ?? host;
}

/** How many fields of the host `id` are edited and not saved. */
export function unsaved(id: string | null): number {
  const draft = id ? drafts[id] : undefined;
  return draft ? changedFields(draft).length : 0;
}

/** Whether the setting `key` of the host `id` is edited and not saved. */
export function settingEdited(id: string, key: keyof Settings): boolean {
  const draft = drafts[id];
  return !!draft && !same(draft.base.settings[key], draft.edited.settings[key]);
}

/** The hosts with unsaved edits. */
export function hostsWithUnsaved(): string[] {
  return Object.keys(drafts).filter((id) => unsaved(id) > 0);
}

/** Why the last Save of the host `id` failed, if it did. */
export function saveError(id: string): string | null {
  return drafts[id]?.error ?? null;
}

/** Edits the host in its draft (made from `host` if there is none). */
export function edit(
  host: Host,
  changes: Partial<Host> | ((current: Host) => Partial<Host>),
) {
  drafts[host.id] ??= {
    base: $state.snapshot(host) as Host,
    edited: $state.snapshot(host) as Host,
    error: null,
  };
  // Read back through the state, so that changes to it are seen.
  const draft = drafts[host.id];
  const current = $state.snapshot(draft.edited) as Host;
  const changed = typeof changes === "function" ? changes(current) : changes;
  draft.edited = { ...current, ...changed, id: host.id };
  draft.error = null;
  // Edited back to what is saved: nothing is left to save.
  if (changedFields(draft).length === 0) delete drafts[host.id];
}

/** Drops the edits of the host `id`. */
export function revert(id: string) {
  delete drafts[id];
}

/** Drops every draft (another workspace opened). */
export function revertAll() {
  for (const id of Object.keys(drafts)) delete drafts[id];
}

/**
 * Saves the edits of the host `id`: its fields, then its name. Resolves to
 * true once everything is saved; on failure the draft stays, with why.
 */
export async function save(id: string): Promise<boolean> {
  const draft = drafts[id];
  if (!draft) return true;
  const fields = changedFields(draft);
  const edited = $state.snapshot(draft.edited) as Host;
  const nameChanged = fields.includes("name");
  const others = fields.filter((field) => field !== "name");
  if (others.length > 0) {
    const error = await saveHostOrSay(id, (current) => {
      const changes: Partial<Host> = {};
      const settings: Settings = { ...current.settings };
      for (const field of others) {
        if (field.startsWith("settings.")) {
          const key = field.slice("settings.".length) as keyof Settings;
          (settings as unknown as Record<string, unknown>)[key] =
            edited.settings[key];
        } else {
          (changes as Record<string, unknown>)[field] =
            edited[field as HostField];
        }
      }
      return { ...changes, settings };
    });
    if (error) {
      if (drafts[id]) drafts[id].error = error;
      return false;
    }
  }
  if (nameChanged) {
    const error = await renameHostOrSay(id, edited.name);
    if (error) {
      // The other fields are saved: what is left to save is the name.
      const tree = inventory.workspace?.tree;
      const saved = tree ? findHost(tree, id) : null;
      if (saved && drafts[id]) {
        drafts[id].base = $state.snapshot(saved) as Host;
        drafts[id].edited = {
          ...($state.snapshot(saved) as Host),
          name: edited.name,
        };
        drafts[id].error = error;
      }
      return false;
    }
  }
  delete drafts[id];
  return true;
}

/** The question about unsaved edits that waits for an answer, if any. */
export const question = $state<{
  current: {
    /** The hosts with unsaved edits. */
    hosts: string[];
    /** What the user is doing, as the question says it ("closing the tab"). */
    doing: string;
    /** Saving failed: why, for the dialog to show. */
    error: string | null;
    answer: (choice: "save" | "discard" | "cancel") => void;
  } | null;
}>({ current: null });

/**
 * Before something that would drop or leave the unsaved edits of `hosts`
 * (closing their tab, showing another host, quitting, another workspace):
 * asks whether to save them, drop them or stay. Resolves to true to go on
 * (saved or dropped), false to stay. Asks nothing when nothing is unsaved.
 */
export async function settleUnsaved(
  hosts: string[],
  doing: string,
): Promise<boolean> {
  const pending = hosts.filter((id) => unsaved(id) > 0);
  if (pending.length === 0) return true;
  let error: string | null = null;
  for (;;) {
    const choice = await new Promise<"save" | "discard" | "cancel">(
      (answer) => (question.current = { hosts: pending, doing, error, answer }),
    );
    question.current = null;
    if (choice === "cancel") return false;
    if (choice === "discard") {
      for (const id of pending) revert(id);
      return true;
    }
    for (const id of pending) await save(id);
    const failed = pending.find((id) => unsaved(id) > 0);
    if (!failed) return true;
    // Asked again, with why: the edits stay until saved or dropped.
    error = saveError(failed);
  }
}
