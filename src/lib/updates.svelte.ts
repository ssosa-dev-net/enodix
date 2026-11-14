// The update check, as the core does it (`netterm_core::update`): once at
// start when the preference allows it, or when the user asks. An installed
// copy offers to install a newer version (only when the user clicks); a
// portable copy only says it exists.

import { invoke } from "@tauri-apps/api/core";

export type InstallKind = "installed" | "portable";

export interface UpdateBuild {
  version: string;
  kind: InstallKind;
  /** The build knows a release server (development builds do not). */
  configured: boolean;
}

export interface UpdateOffer {
  version: string;
  current: string;
  notes: string | null;
  published: string | null;
  page: string | null;
  action: "install" | "notify";
}

export type UpdateStatus =
  | { kind: "disabled" }
  | { kind: "notConfigured" }
  | { kind: "upToDate"; current: string }
  | ({ kind: "available" } & UpdateOffer)
  | { kind: "failed"; reason: string };

export const updates = $state({
  build: null as UpdateBuild | null,
  /** The last check's outcome (null: none made yet). */
  status: null as UpdateStatus | null,
  /** A check or an install is under way. */
  busy: false,
  /** The user closed the notice of this version ("Later"). */
  dismissed: null as string | null,
  /** Why the last install or page opening failed. */
  error: null as string | null,
});

/** The offer to show, if any (not dismissed). */
export function shownOffer(): (UpdateOffer & { kind: "available" }) | null {
  const status = updates.status;
  if (status?.kind !== "available") return null;
  return updates.dismissed === status.version ? null : status;
}

async function check(command: string) {
  updates.busy = true;
  updates.error = null;
  try {
    updates.status = await invoke<UpdateStatus>(command);
  } catch (error) {
    updates.status = { kind: "failed", reason: String(error) };
  } finally {
    updates.busy = false;
  }
}

/** At start: learns about the build, then checks if the user allows it
 * (the core makes no connection otherwise). */
export async function startUpdates() {
  try {
    updates.build = await invoke<UpdateBuild>("update_build");
  } catch {
    // Without it the preferences only show less.
  }
  await check("update_check_automatic");
}

/** A check the user asked for. */
export function checkNow() {
  updates.dismissed = null;
  return check("update_check_now");
}

/** Installs the offered version: the app ends and the installer runs. */
export async function installUpdate() {
  updates.busy = true;
  updates.error = null;
  try {
    await invoke("update_install");
  } catch (error) {
    updates.error = String(error);
  } finally {
    updates.busy = false;
  }
}

export async function openReleasePage() {
  updates.error = null;
  try {
    await invoke<string>("update_open_page");
  } catch (error) {
    updates.error = String(error);
  }
}

/** Plain words for a status, for the preferences dialog. */
export function describeStatus(status: UpdateStatus | null): string {
  switch (status?.kind) {
    case undefined:
      return "";
    case "disabled":
      return "Not checked at start.";
    case "notConfigured":
      return "This build has no release server to ask.";
    case "upToDate":
      return `Up to date (${status.current}).`;
    case "available":
      return `Version ${status.version} is available.`;
    case "failed":
      return `The check failed: ${status.reason}`;
  }
}
