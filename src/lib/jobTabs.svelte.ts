// What the job tabs show (#69). A job tab is opened before its job has a
// run (at the choice of snippet or script, or at pre-flight), so its
// subject is a key of its own (`new-<n>`) naming a draft kept here: what it
// runs on and what it runs. Once started, the draft knows its run. A past
// run's tab has the run id as its subject. `views.ts` opens both kinds.

import {
  forgetPlan,
  stepTitle,
  type JobPlan,
  type JobTarget,
  type ShownPlan,
} from "./jobs";

/** What a job tab runs. */
export type JobKind = "snippet" | "script" | "backup" | "enrich" | "file";

/** A job tab's job, from when it opens. */
export interface JobDraft {
  kind: JobKind;
  /** The hosts to run on. */
  target: JobTarget;
  /** What the target is called (a folder, a saved filter, a host). */
  label: string;
  /** A plan already made (Run again, a saved job): starts at pre-flight. */
  planned: ShownPlan | null;
  /** The id of the plan its pre-flight shows now, which the shell keeps
   * until the job starts or the tab closes (#121). */
  shown: number | null;
  /** The run it keeps, once started. */
  run: string | null;
  /** Whether its tab has been seen in the layout (so a missing tab means
   * it closed, not that it is still opening). */
  opened: boolean;
}

/** The open job tabs' drafts, by tab subject. */
export const jobDrafts = $state<Record<string, JobDraft>>({});

/** What each run is called (its snippet, script, "Config backup"), as
 * listed; for the run tabs' names. */
export const runTitles = $state<Record<string, string>>({});

let next = 1;

/** Keeps `draft` under a new subject, which it returns. */
export function addDraft(
  draft: Omit<JobDraft, "run" | "opened" | "shown">,
): string {
  const key = `new-${next++}`;
  jobDrafts[key] = {
    ...draft,
    shown: draft.planned?.id ?? null,
    run: null,
    opened: false,
  };
  return key;
}

/**
 * Notes that the job tab `subject` shows the plan `plan` now: the one it
 * showed before (planned again) is forgotten by the shell.
 */
export function showPlan(subject: string, plan: number) {
  const draft = jobDrafts[subject];
  if (!draft) {
    void forgetPlan(plan);
    return;
  }
  if (draft.shown !== null && draft.shown !== plan)
    void forgetPlan(draft.shown);
  draft.shown = plan;
}

/** Notes that the job tab `subject` started its plan (the shell forgot
 * it), keeping `run`. */
export function started(subject: string, run: string) {
  const draft = jobDrafts[subject];
  if (!draft) return;
  draft.run = run;
  draft.shown = null;
}

/** The kind of job `plan` runs. */
export function kindOf(plan: JobPlan): JobKind {
  switch (plan.step.kind) {
    case "configBackup":
      return "backup";
    case "enrich":
      return "enrich";
    case "script":
      return "script";
    case "sendFile":
      return "file";
    default:
      return "snippet";
  }
}

/** A job tab's name. */
export function draftTitle(subject: string | null): string {
  const draft = subject === null ? undefined : jobDrafts[subject];
  if (!draft) return "Job";
  if (draft.kind === "backup") return `Back up ${draft.label}`;
  if (draft.kind === "enrich") return `Fill in ${draft.label}`;
  if (draft.kind === "file" && !draft.planned)
    return `Send file to ${draft.label}`;
  const what = draft.planned
    ? stepTitle(draft.planned.step)
    : draft.kind === "script"
      ? "Script"
      : "Snippet";
  return `${what} on ${draft.label}`;
}

/** A past run's tab name. */
export function runTitle(subject: string | null): string {
  return (subject && runTitles[subject]) || `Run ${subject ?? ""}`.trim();
}

/** The subject of the job tab that started `run`, if one is open. */
export function draftOfRun(run: string): string | null {
  return (
    Object.keys(jobDrafts).find((key) => jobDrafts[key].run === run) ?? null
  );
}

/**
 * Forgets the drafts of job tabs no longer open; `open` = the subjects of
 * the job tabs in the layout. A draft whose tab is not there yet (opening)
 * is kept. The shell forgets the plan a closed tab still showed.
 */
export function forgetClosedDrafts(open: Set<string>) {
  for (const key of Object.keys(jobDrafts)) {
    if (open.has(key)) jobDrafts[key].opened = true;
    else if (jobDrafts[key].opened) {
      const shown = jobDrafts[key].shown;
      if (shown !== null) void forgetPlan(shown);
      delete jobDrafts[key];
    }
  }
}
