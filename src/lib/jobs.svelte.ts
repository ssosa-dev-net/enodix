// The jobs this window started, with their progress, kept apart from the
// tabs that show them: closing a job's tab leaves it running (the shell
// drives it and keeps its run on disk), and the rail's Jobs icon counts the
// running ones.

import { inventory } from "./inventory.svelte";
import {
  startJob,
  type HostState,
  type JobEvent,
  type JobPlan,
  type JobTarget,
  type SaveOutcome,
} from "./jobs";

/** A job started from this window, and what it has reported so far. */
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
}

export const liveJobs = $state({
  jobs: [] as LiveJob[],
  /** +1 whenever a job starts or finishes, or a run is deleted (the run
   * lists refetch). */
  revision: 0,
});

/** The open workspace's root, or "" with none. */
function openRoot(): string {
  return inventory.workspace?.root ?? "";
}

/**
 * How many jobs started here are still running: in the open workspace,
 * or (`elsewhere`) in the others.
 */
export function runningCount(elsewhere = false): number {
  return liveJobs.jobs.filter(
    (job) => !job.finished && (job.workspace === openRoot()) !== elsewhere,
  ).length;
}

/** The job started here that keeps the open workspace's run `run`. */
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
  } else {
    for (const host of event.hosts) {
      job.states[host.id] = host.state;
      job.outputs[host.id] = host.output;
      if (host.backup) job.backups[host.id] = host.backup;
      if (host.exit_status !== null) job.exits[host.id] = host.exit_status;
    }
    job.finished = true;
    liveJobs.revision += 1;
  }
}

/**
 * Starts the job last planned (`plan`, on `target`), at most `parallel`
 * hosts at once; its progress is kept here. Errors propagate.
 */
export async function launchJob(
  plan: JobPlan,
  target: JobTarget,
  label: string,
  parallel: number,
): Promise<LiveJob> {
  let job: LiveJob | null = null;
  // Events can arrive before the start answers.
  const early: JobEvent[] = [];
  const started = await startJob(parallel, (event) => {
    if (job) apply(job, event);
    else early.push(event);
  });
  liveJobs.jobs.push({
    id: started.id,
    workspace: openRoot(),
    run: started.run,
    label,
    target,
    plan,
    states: {},
    outputs: {},
    backups: {},
    exits: {},
    finished: false,
  });
  // The proxied entry, so that changes show.
  job = liveJobs.jobs[liveJobs.jobs.length - 1];
  for (const event of early) apply(job, event);
  liveJobs.revision += 1;
  return job;
}

/** Forgets finished jobs other than `keep` (their runs stay on disk). */
export function forgetFinished(keep: string | null = null) {
  liveJobs.jobs = liveJobs.jobs.filter(
    (job) => !job.finished || job.run === keep,
  );
}
