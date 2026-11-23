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

import {
  inventory,
  onHostsMoved,
  renameHostOrSay,
  saveHostOrSay,
} from "./inventory.svelte";
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

/** The fields that differ between two versions of a host: host fields by
 * name, settings as `settings.<key>`. */
function differing(a: Host, b: Host): string[] {
  const fields: string[] = HOST_FIELDS(b).filter(
    (key) => !same(a[key], b[key]),
  );
  const keys = new Set([
    ...Object.keys(a.settings),
    ...Object.keys(b.settings),
  ]) as Set<keyof Settings>;
  for (const key of keys) {
    if (!same(a.settings[key], b.settings[key])) fields.push(`settings.${key}`);
  }
  return fields;
}

/** The fields the draft changes. */
function changedFields(draft: Draft): string[] {
  return differing(draft.base, draft.edited);
}

/** `target` with the `fields` (as `differing` names them) of `source`. */
function withFields(target: Host, source: Host, fields: string[]): Host {
  const result: Host = { ...target, settings: { ...target.settings } };
  for (const field of fields) {
    if (field.startsWith("settings.")) {
      const key = field.slice("settings.".length) as keyof Settings;
      (result.settings as unknown as Record<string, unknown>)[key] =
        source.settings[key];
    } else {
      (result as unknown as Record<string, unknown>)[field] =
        source[field as HostField];
    }
  }
  return result;
}

function savedHost(id: string): Host | null {
  const tree = inventory.workspace?.tree;
  const found = tree ? findHost(tree, id) : null;
  return found ? ($state.snapshot(found) as Host) : null;
}

/** Where hosts went when renamed or moved in the app, so that a save
 * waiting on a rename finds its draft again. */
const movedTo = new Map<string, string>();

// A host renamed or moved (its own rename, the tree, a folder's move)
// keeps its edits under its new id. Listened to from the first edit: this
// module is loaded while inventory's is (they import each other).
let listening = false;
function followMoves() {
  if (listening) return;
  listening = true;
  onHostsMoved((moves) => {
    for (const { from, to } of moves) {
      movedTo.set(from, to);
      const draft = drafts[from];
      if (!draft || from === to) continue;
      const copy = $state.snapshot(draft) as Draft;
      delete drafts[from];
      const saved = savedHost(to);
      // A name not being edited follows the host's new name.
      const name =
        copy.edited.name === copy.base.name && saved ? saved.name : null;
      drafts[to] = {
        base: { ...copy.base, id: to, ...(name ? { name } : {}) },
        edited: { ...copy.edited, id: to, ...(name ? { name } : {}) },
        error: copy.error,
      };
    }
  });
}

/** The id the host `id` has now, after the moves seen since. */
function current(id: string): string {
  const seen = new Set<string>();
  while (movedTo.has(id) && !seen.has(id)) {
    seen.add(id);
    id = movedTo.get(id)!;
  }
  return id;
}

/** The host `id` as the editor shows it: its draft, else as saved. */
export function shown(host: Host): Host {
  return drafts[host.id]?.edited ?? host;
}

/** Whether the host `id` is in the workspace. */
function exists(id: string): boolean {
  const tree = inventory.workspace?.tree;
  return !!tree && !!findHost(tree, id);
}

/** How many fields of the host `id` are edited and not saved. Edits to a
 * host that is gone (deleted, or removed by a git pull) count as none:
 * there is nothing left to save them to, so nothing asks about them. */
export function unsaved(id: string | null): number {
  const draft = id ? drafts[id] : undefined;
  return draft && exists(id!) ? changedFields(draft).length : 0;
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
  followMoves();
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
 * After a save of `written` (the edited host as it was sent): the draft of
 * the host `id` now starts from the host as saved, keeping only the edits
 * made while the save was on its way; with none, it goes.
 */
function settle(id: string, written: Host) {
  const draft = drafts[id];
  if (!draft) return;
  const saved = savedHost(id);
  if (!saved) {
    delete drafts[id];
    return;
  }
  const edited = $state.snapshot(draft.edited) as Host;
  const since = differing({ ...written, id }, { ...edited, id });
  draft.base = saved;
  draft.edited = withFields(saved, edited, since);
  draft.error = null;
  if (changedFields(draft).length === 0) delete drafts[id];
}

/**
 * Saves the edits of the host `id`: its fields, then its name. Resolves to
 * true once everything is saved; on failure the draft stays, with why.
 * Edits made while it saves stay in the draft, unsaved.
 */
export async function save(id: string): Promise<boolean> {
  const draft = drafts[id];
  if (!draft) return true;
  const fields = changedFields(draft);
  const written = $state.snapshot(draft.edited) as Host;
  const nameChanged = fields.includes("name");
  const others = fields.filter((field) => field !== "name");
  if (others.length > 0) {
    const error = await saveHostOrSay(id, (saved) => {
      const changes = withFields(saved, written, others);
      return { ...changes, name: saved.name, id: saved.id };
    });
    if (error) {
      if (drafts[id]) drafts[id].error = error;
      return false;
    }
  }
  if (nameChanged) {
    const error = await renameHostOrSay(id, written.name);
    if (error) {
      // The other fields are saved: what is left is the name, and any
      // edits made meanwhile.
      settle(id, { ...written, name: savedHost(id)?.name ?? written.name });
      if (drafts[id]) drafts[id].error = error;
      return false;
    }
  }
  settle(current(id), written);
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
 * asks whether to save them, discard them or stay. Resolves to false to
 * stay, else to a function that the caller runs once it really goes on:
 * discarded edits are dropped only then, so that a later Cancel (of a
 * session question, say) keeps them. Asks nothing when nothing is unsaved.
 */
export async function settleUnsaved(
  hosts: string[],
  doing: string,
): Promise<false | (() => void)> {
  const pending = hosts.filter((id) => unsaved(id) > 0);
  if (pending.length === 0) return () => {};
  // One question at a time: an earlier one still open is cancelled.
  question.current?.answer("cancel");
  let error: string | null = null;
  for (;;) {
    let asked: NonNullable<typeof question.current> | null = null;
    const choice = await new Promise<"save" | "discard" | "cancel">(
      (answer) => {
        asked = { hosts: pending, doing, error, answer };
        question.current = asked;
      },
    );
    // A later question may already have taken the slot.
    if (question.current === asked) question.current = null;
    if (choice === "cancel") return false;
    if (choice === "discard") {
      return () => {
        for (const id of pending) revert(current(id));
      };
    }
    for (const id of pending) await save(current(id));
    const failed = pending.map(current).find((id) => unsaved(id) > 0);
    if (!failed) return () => {};
    // Asked again, with why: the edits stay until saved or discarded.
    error = saveError(failed);
  }
}
