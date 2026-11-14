// The Python environment, as the core checks it (`netterm_core::python`):
// which interpreter the Python features use, and the app's own environment
// set up on request. Until an interpreter is chosen every Python feature is
// off: a feature asks `pythonOff()` and shows its message (which says how
// to turn them on) instead of running.

import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

export interface PythonVersion {
  major: number;
  minor: number;
  patch: number;
}

export type Setup = "managed" | "own";

export type PythonProblem =
  | { kind: "missing" }
  | { kind: "notPython"; reason: string }
  | { kind: "tooOld"; version: PythonVersion }
  | { kind: "packageMissing"; version: PythonVersion; reason: string | null }
  | {
      kind: "packageMismatch";
      version: PythonVersion;
      installed: string;
      expected: string;
    };

export type PythonStatus =
  | { kind: "notSet"; message: string }
  | {
      kind: "ready";
      interpreter: string;
      setup: Setup;
      version: PythonVersion;
      package: string;
      message: string;
    }
  | {
      kind: "problem";
      interpreter: string;
      setup: Setup;
      problem: PythonProblem;
      message: string;
    };

export interface PythonView {
  status: PythonStatus;
  /** Where the app's own environment is (or would be). */
  environment: string;
  /** The app's wheel beside the program (null: not found). */
  wheel: string | null;
}

export interface Interpreter {
  executable: string;
  version: PythonVersion;
  package: string | null;
}

export type Library = "nornir" | "netmiko" | "scrapli" | "napalm";

export interface SetupOffer {
  base: Interpreter;
  environment: string;
  existing: boolean;
  wheel: string;
  libraries: { library: Library; label: string; purpose: string }[];
}

export interface PlannedInstall {
  name: string;
  source: { kind: "wheel"; path: string } | { kind: "index" };
}

export interface SetupPlan {
  base: Interpreter;
  environment: string;
  interpreter: string;
  existing: boolean;
  libraries: Library[];
  installs: PlannedInstall[];
}

export type SetupProgress =
  { kind: "step"; description: string } | { kind: "output"; line: string };

export const python = $state({
  view: null as PythonView | null,
  /** A check is under way. */
  busy: false,
  error: null as string | null,
  /** The setup dialog is open. */
  dialog: false,
});

export function versionText(version: PythonVersion): string {
  return `${version.major}.${version.minor}.${version.patch}`;
}

/**
 * Why the Python features cannot run now (with how to fix it), or null
 * when they can.
 */
export function pythonOff(): string | null {
  const status = python.view?.status;
  if (!status)
    return "Python features are off until the interpreter is checked.";
  return status.kind === "ready" ? null : status.message;
}

async function adopt(command: string, args?: Record<string, unknown>) {
  python.busy = true;
  python.error = null;
  try {
    python.view = await invoke<PythonView>(command, args);
  } catch (error) {
    python.error = String(error);
  } finally {
    python.busy = false;
  }
}

/** Checks the chosen interpreter again. */
export function refreshPython(): Promise<void> {
  return adopt("python_status");
}

/** Stores the user's own interpreter (nothing is installed) and checks it. */
export function useInterpreter(path: string): Promise<void> {
  return adopt("python_use_interpreter", { path });
}

/** Forgets the choice: the Python features are off again. */
export function turnOffPython(): Promise<void> {
  return adopt("python_turn_off");
}

/** Asks the user for an interpreter's program; null if they cancel. */
export async function pickInterpreter(): Promise<string | null> {
  const file = await open({
    title: "Choose a Python interpreter",
  });
  return typeof file === "string" ? file : null;
}

/** Looks for a Python on the system for the app's environment. */
export function findPython(): Promise<SetupOffer> {
  return invoke<SetupOffer>("python_offer");
}

/** Everything the setup with these libraries will install. */
export function planSetup(libraries: Library[]): Promise<SetupPlan> {
  return invoke<SetupPlan>("python_plan", { libraries });
}

/** Runs the plan the user confirmed; the new state replaces the shown one. */
export async function runSetup(
  onprogress: (progress: SetupProgress) => void,
): Promise<PythonView> {
  const onProgress = new Channel<SetupProgress>();
  onProgress.onmessage = onprogress;
  const view = await invoke<PythonView>("python_setup", { onProgress });
  python.view = view;
  python.error = null;
  return view;
}

export function cancelSetup(): Promise<void> {
  return invoke("python_setup_cancel");
}
