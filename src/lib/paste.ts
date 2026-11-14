// Paste safety, as the core decides it (`netterm_core::paste`): how a
// paste or snippet is split into lines, whether the user confirms it first,
// and the delay between its lines.

import { invoke } from "@tauri-apps/api/core";
import type { TabId } from "./layout";
import type { Environment, PasteSafety } from "./workspace";

export interface PastePlan {
  /** Each with its own line break; the last one may have none. */
  lines: string[];
  /** Whether the user must confirm it before any line is sent. */
  confirm: boolean;
  /** Milliseconds to wait between two lines. */
  lineDelayMs: number;
}

/** For sessions without an inventory host (quick connect). */
export const NO_SAFETY: PasteSafety = { environment: null, lineDelayMs: 0 };

export const ENVIRONMENT_NAMES: Record<Environment, string> = {
  production: "Production",
  staging: "Staging",
  lab: "Lab",
};

/** How one paste goes out to the multi-send group's members. */
export interface GroupPastePlan {
  /** Each member's own plan, in the order the members were given. */
  members: PastePlan[];
  /** Whether the user confirms it once, before any member gets a line. */
  confirm: boolean;
  /** The strictest member environment, for how the question looks. */
  environment: Environment | null;
}

/** How a paste goes out to the group: each member's text and safety. */
export function planGroupPaste(
  members: { text: string; safety: PasteSafety }[],
): Promise<GroupPastePlan> {
  return invoke<GroupPastePlan>("paste_plan_group", { members });
}

/**
 * Which of the tabs about to join the multi-send group the user confirms
 * first (the core decides: production ones).
 */
export function joinConfirmations(
  joining: { tab: TabId; safety: PasteSafety }[],
): Promise<TabId[]> {
  return invoke<TabId[]>("group_join_check", { joining });
}

/** How `text` is pasted into a session with `safety`. */
export function planPaste(
  text: string,
  safety: PasteSafety,
): Promise<PastePlan> {
  return invoke<PastePlan>("paste_plan", { text, safety });
}
