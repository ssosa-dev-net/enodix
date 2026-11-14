// Paste safety, as the core decides it (`netterm_core::paste`): how a
// paste or snippet is split into lines, whether the user confirms it first,
// and the delay between its lines.

import { invoke } from "@tauri-apps/api/core";
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

/** How `text` is pasted into a session with `safety`. */
export function planPaste(
  text: string,
  safety: PasteSafety,
): Promise<PastePlan> {
  return invoke<PastePlan>("paste_plan", { text, safety });
}
