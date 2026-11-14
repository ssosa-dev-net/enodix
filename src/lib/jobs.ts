// Jobs: a saved snippet run on many hosts at once by the core, over its
// own headless sessions (see the core's `job` module). The interface only
// shows the pre-flight list and the progress.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Environment } from "./workspace";

/** Which hosts a job runs on. */
export type JobTarget =
  { kind: "folder"; path: string } | { kind: "hosts"; ids: string[] };

/** What a job does on each host. */
export interface JobStep {
  kind: "snippet";
  name: string;
  /** Values for the variables no host fills, the same for every host. */
  answers: Record<string, string>;
}

export interface PlannedHost {
  id: string;
  name: string;
  address: string;
  environment: Environment | null;
  /** The platform definition that drives it. */
  platform: string;
}

export interface SkippedHost {
  id: string;
  name: string;
  reason: { kind: string };
  /** Why, for the user. */
  message: string;
}

/** The pre-flight list. */
export interface JobPlan {
  step: JobStep;
  hosts: PlannedHost[];
  skipped: SkippedHost[];
  /** The production hosts that will run: confirmed once. */
  production: string[];
  /** Variables to ask for once before the job can start. */
  missingVariables: string[];
}

export type HostState =
  | { kind: "waiting" }
  | { kind: "connecting" }
  | { kind: "running" }
  | { kind: "done" }
  | { kind: "failed"; reason: string }
  | { kind: "cancelled" };

export interface HostResult {
  id: string;
  name: string;
  state: HostState;
  output: string;
}

export type JobEvent =
  | { kind: "state"; host: string; state: HostState }
  | { kind: "output"; host: string; text: string }
  | { kind: "finished"; hosts: HostResult[] };

/** Works out the pre-flight list (connects to nothing). */
export function planJob(target: JobTarget, step: JobStep) {
  return invoke<JobPlan>("job_plan", { target, step });
}

/** How many hosts a job runs at once unless the user says otherwise. */
export const DEFAULT_PARALLEL = 10;
/** The most hosts a job may run at once. */
export const MAX_PARALLEL = 50;

/**
 * Starts the job last planned, at most `parallel` hosts at once (1 to
 * `MAX_PARALLEL`); returns its id.
 */
export function startJob(parallel: number, onEvent: (event: JobEvent) => void) {
  return invoke<number>("job_start", {
    parallel,
    onEvent: new Channel<JobEvent>(onEvent),
  });
}

/** Cancels a running job, keeping the output collected. */
export function cancelJob(id: number) {
  return invoke<void>("job_cancel", { id });
}
