// The jobs started in this app run, with their progress, kept apart from
// the tabs that show them: closing a job's tab leaves it running (the shell
// drives it and keeps its run on disk), and the rail's Jobs icon counts the
// running ones.
//
// The shell keeps the record (#77, the core's `job::LiveJobs`) for every
// window: it announces each job as it starts (`job-started`) and
// broadcasts its events numbered (`job-event`). A window reads the record
// when it starts (`startLiveJobs`), so a reloaded page or a detached
// window opened late shows what already happened, then applies only the
// events numbered after what it read.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inventory } from "./inventory.svelte";
import {
  startJob,
  type HostState,
  type JobEvent,
  type JobPlan,
  type JobTarget,
  type SaveOutcome,
} from "./jobs";

/** A job started in this app run, and what it has reported so far. */
export interface LiveJob {
  id: number;
  /** The root of the workspace it runs in (run ids are per workspace). */
  workspace: string;
  /** Its kept run. */
  run: string;
  /** What it runs on, for the user. */
  label: string;
  target: JobTarget;
  plan: JobPlan;
  states: Record<string, HostState>;
  outputs: Record<string, string>;
  /** What a config backup kept, per host. */
  backups: Record<string, SaveOutcome>;
  /** A script's exit status, once it ended with one. */
  exits: Record<string, number>;
  finished: boolean;
  /** The number of the latest event it holds. */
  events: number;
}

/** A job's event as the shell broadcasts it. */
interface JobBroadcast {
  id: number;
  /** Counts from 1 per job. */
  number: number;
  event: JobEvent;
}

export const liveJobs = $state({
  jobs: [] as LiveJob[],
  /** +1 whenever a job starts or finishes, or a run is deleted (the run
   * lists refetch). */
  revision: 0,
  /** +1 whenever a backup keeps or checks a configuration (a job's, or a
   * backup on login): the config views refetch. */
  configs: 0,
});

/** Notes that a backup kept or checked a configuration. */
export function noteBackup() {
  liveJobs.configs += 1;
}

/** The open workspace's root, or "" with none. */
function openRoot(): string {
  return inventory.workspace?.root ?? "";
}

/**
 * How many jobs are still running: in the open workspace, or
 * (`elsewhere`) in the others.
 */
export function runningCount(elsewhere = false): number {
  return liveJobs.jobs.filter(
    (job) => !job.finished && (job.workspace === openRoot()) !== elsewhere,
  ).length;
}

/** The live job that keeps the open workspace's run `run`. */
export function liveJob(run: string): LiveJob | undefined {
  return liveJobs.jobs.find(
    (job) => job.run === run && job.workspace === openRoot(),
  );
}

function apply(job: LiveJob, event: JobEvent) {
  if (event.kind === "state") {
    job.states[event.host] = event.state;
  } else if (event.kind === "output") {
    job.outputs[event.host] = (job.outputs[event.host] ?? "") + event.text;
  } else if (event.kind === "backup") {
    job.backups[event.host] = event.outcome;
    noteBackup();
  } else {
    for (const host of event.hosts) {
      job.states[host.id] = host.state;
      job.outputs[host.id] = host.output;
      if (host.backup) job.backups[host.id] = host.backup;
      if (host.exit_status !== null) job.exits[host.id] = host.exit_status;
    }
    job.finished = true;
    liveJobs.revision += 1;
    noteBackup();
  }
}

/** Takes in a job's record, unless the one held is as new. */
function adopt(record: LiveJob) {
  const index = liveJobs.jobs.findIndex((job) => job.id === record.id);
  if (index < 0) {
    liveJobs.jobs.push(record);
    liveJobs.revision += 1;
  } else if (liveJobs.jobs[index].events < record.events) {
    // In place: tabs hold the entry.
    Object.assign(liveJobs.jobs[index], record);
    liveJobs.revision += 1;
  }
}

/** Applies a broadcast event once, in order. */
function receive({ id, number, event }: JobBroadcast) {
  const job = liveJobs.jobs.find((entry) => entry.id === id);
  // Already in the record read (or one missed: the record is read again).
  if (!job || number <= job.events) return;
  if (number !== job.events + 1) {
    void reload();
    return;
  }
  job.events = number;
  apply(job, event);
}

async function reload() {
  const records = await invoke<LiveJob[]>("jobs_live");
  for (const record of records) adopt(record);
}

let started: Promise<void> | null = null;

/**
 * Follows the shell's record of live jobs: listens first, then reads it,
 * so nothing falls between. Events of jobs not yet read wait for it. Once
 * per window; the window lives as long as the listeners.
 */
export function startLiveJobs(): Promise<void> {
  started ??= (async () => {
    let loaded = false;
    const early: JobBroadcast[] = [];
    await listen<LiveJob>("job-started", (event) => adopt(event.payload));
    await listen<JobBroadcast>("job-event", (event) => {
      if (loaded) receive(event.payload);
      else early.push(event.payload);
    });
    await reload();
    loaded = true;
    for (const broadcast of early) receive(broadcast);
  })();
  return started;
}

/**
 * Starts the job last planned (`plan`, on `target`), at most `parallel`
 * hosts at once; its progress is kept here, as in every window. Errors
 * propagate.
 */
export async function launchJob(
  plan: JobPlan,
  target: JobTarget,
  label: string,
  parallel: number,
): Promise<LiveJob> {
  void startLiveJobs();
  const begun = await startJob(parallel, label);
  // Its announcement can come after the answer.
  let job = liveJobs.jobs.find((entry) => entry.id === begun.id);
  if (!job) {
    adopt({
      id: begun.id,
      workspace: openRoot(),
      run: begun.run,
      label,
      target,
      plan,
      states: {},
      outputs: {},
      backups: {},
      exits: {},
      finished: false,
      events: 0,
    });
    job = liveJobs.jobs.find((entry) => entry.id === begun.id)!;
  }
  return job;
}

/** Forgets finished jobs other than `keep`, here and in the shell's
 * record (their runs stay on disk). */
export function forgetFinished(keep: string | null = null) {
  liveJobs.jobs = liveJobs.jobs.filter(
    (job) => !job.finished || job.run === keep,
  );
  void invoke("jobs_forget_finished", { keep });
}
