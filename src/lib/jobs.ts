// Jobs: a saved snippet run on many hosts at once by the core, over its
// own headless sessions (see the core's `job` module). The interface only
// shows the pre-flight list and the progress.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Environment } from "./workspace";

/** Which hosts a job runs on. */
export type JobTarget =
  { kind: "folder"; path: string } | { kind: "hosts"; ids: string[] };

/** What a job does on each host. */
export type JobStep =
  | {
      kind: "snippet";
      name: string;
      /** Values for the variables no host fills, the same for every host. */
      answers: Record<string, string>;
    }
  /** Fetches each configuration and keeps it in the config history. */
  | { kind: "configBackup" };

/** What a config backup kept of one host's configuration. */
export type SaveOutcome =
  /** Changed (or the first): kept as a new version. */
  | { kind: "saved"; version: string }
  /** The same as the latest version, apart from ignored lines. */
  | { kind: "unchanged"; version: string };

/** What a kept run is called: its snippet, or "Config backup". */
export function stepTitle(step: JobStep): string {
  return step.kind === "snippet" ? step.name : "Config backup";
}

/** A config backup's outcome, for the user. */
export function outcomeText(outcome: SaveOutcome): string {
  const when = versionTime(outcome.version);
  return outcome.kind === "saved"
    ? `Saved as version ${when}`
    : `Unchanged since version ${when}`;
}

/** A version's name (`YYYY-MM-DD_HH-MM-SS[_n]`) as a date and time. */
export function versionTime(version: string): string {
  const match = /^(\d{4}-\d{2}-\d{2})_(\d{2})-(\d{2})-(\d{2})(?:_(\d+))?$/.exec(
    version,
  );
  if (!match) return version;
  const [, day, h, m, s, n] = match;
  return `${day} ${h}:${m}:${s}${n ? ` (${n})` : ""}`;
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
  /** For a config backup that got the configuration. */
  backup: SaveOutcome | null;
}

export type JobEvent =
  | { kind: "state"; host: string; state: HostState }
  | { kind: "output"; host: string; text: string }
  | { kind: "backup"; host: string; outcome: SaveOutcome }
  | { kind: "finished"; hosts: HostResult[] };

/** Works out the pre-flight list (connects to nothing). */
export function planJob(target: JobTarget, step: JobStep) {
  return invoke<JobPlan>("job_plan", { target, step });
}

/** How many hosts a job runs at once unless the user says otherwise. */
export const DEFAULT_PARALLEL = 10;
/** The most hosts a job may run at once. */
export const MAX_PARALLEL = 50;

/** A started job: its id, and its kept run's. */
export interface StartedJob {
  id: number;
  run: string;
}

/**
 * Starts the job last planned, at most `parallel` hosts at once (1 to
 * `MAX_PARALLEL`), keeping its run on disk.
 */
export function startJob(
  parallel: number,
  onEvent: (event: JobEvent) => void,
): Promise<StartedJob> {
  return invoke<StartedJob>("job_start", {
    parallel,
    onEvent: new Channel<JobEvent>(onEvent),
  });
}

/** A host of a kept run. */
export interface RunHost {
  id: string;
  name: string;
  address: string;
  state: HostState;
  /** Its output file, relative to the run folder. */
  file: string;
  /** For a config backup: the version holding its configuration. */
  backup?: SaveOutcome;
}

/** A kept run, as its description says. */
export interface StoredRun {
  id: string;
  workspace: string;
  target: JobTarget;
  step: JobStep;
  /** RFC 3339, local time. */
  started: string;
  /** null while it runs, or if the app closed first. */
  ended: string | null;
  hosts: RunHost[];
  skipped: { id: string; name: string; reason: string }[];
}

export interface RunList {
  /** Newest first. */
  runs: StoredRun[];
  /** Run folders whose description cannot be read. */
  damaged: { id: string; reason: string }[];
  /** The runs whose jobs still run. */
  running: string[];
}

/** One host's output in a kept run. */
export interface OutputRef {
  run: string;
  host: string;
}

export interface SearchHit {
  host: string;
  name: string;
  count: number;
  lines: { number: number; text: string }[];
}

export type DiffLine =
  | { kind: "same"; old: number; new: number; text: string }
  | { kind: "removed"; old: number; text: string }
  | { kind: "added"; new: number; text: string };

/** The open workspace's kept runs, newest first. */
export function listRuns() {
  return invoke<RunList>("job_runs");
}

export function readRun(run: string) {
  return invoke<StoredRun>("job_run", { run });
}

export function runOutput(output: OutputRef) {
  return invoke<string>("job_run_output", { output });
}

/** The hosts of a run whose output holds `text`, ignoring case. */
export function searchRun(run: string, text: string) {
  return invoke<SearchHit[]>("job_run_search", { run, text });
}

/** A line diff of two hosts' output, `old` first. */
export function compareOutputs(old: OutputRef, new_: OutputRef) {
  return invoke<DiffLine[]>("job_compare", { old, new: new_ });
}

/** Copies a run into `folder`; the copy's path. */
export function exportRun(run: string, folder: string) {
  return invoke<string>("job_run_export", { run, folder });
}

/** Deletes a run from disk (refused while it runs). */
export function deleteRun(run: string) {
  return invoke<void>("job_run_delete", { run });
}

/** Plans a kept run again on its target, as the workspace is now. */
export function planAgain(run: string) {
  return invoke<{ target: JobTarget; plan: JobPlan }>("job_run_again", {
    run,
  });
}

/** A kept version of a host's configuration. */
export interface ConfigVersion {
  /** Its name: the local time it was saved, `YYYY-MM-DD_HH-MM-SS[_n]`. */
  id: string;
  lines: number;
  bytes: number;
}

/** The kept versions of a host's configuration, newest first. */
export function configVersions(host: string) {
  return invoke<ConfigVersion[]>("config_versions", { host });
}

export function configVersion(host: string, version: string) {
  return invoke<string>("config_version", { host, version });
}

/** A line diff of two versions of a host's configuration, `old` first. */
export function diffConfigs(host: string, old: string, new_: string) {
  return invoke<DiffLine[]>("config_diff", { host, old, new: new_ });
}

/** Copies a version into `folder`; the copy's path. */
export function exportConfig(host: string, version: string, folder: string) {
  return invoke<string>("config_export", { host, version, folder });
}

/** Cancels a running job, keeping the output collected. */
export function cancelJob(id: number) {
  return invoke<void>("job_cancel", { id });
}
