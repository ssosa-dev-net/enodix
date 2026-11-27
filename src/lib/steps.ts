// A snippet's steps on the open session (#23), as the core runs them
// (`enodix_core::steps`): what they will do, for the user to check, then
// the run's progress until it ends.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Environment, SnippetSteps } from "./workspace";

/** What the steps of a snippet will do on a session, or what to ask. */
export type StepsPreview =
  | {
      kind: "ready";
      /** One line each: the answers and stops, then each step. */
      preview: string[];
      count: number;
      /** Whether the user confirms them first (always on production). */
      confirm: boolean;
      environment: Environment | null;
      /** What identifies the steps shown: given back to run them. */
      digest: string;
    }
  | { kind: "missing"; names: string[] };

/** Who called a run off. */
export type CalledOff = "user" | "input" | "paste";

/** How a run ended. */
export type RunEnd =
  | { kind: "finished" }
  | {
      kind: "stopped";
      step: number;
      line: string | null;
      pattern: string;
      shown: string;
    }
  | {
      kind: "timedOut";
      step: number;
      line: string | null;
      waitingFor: string;
    }
  | { kind: "calledOff"; step: number; line: string | null; by: CalledOff }
  | { kind: "sessionEnded"; step: number; line: string | null };

/** What a run tells while it goes; `ended` is always the last. */
export type RunEvent =
  | { kind: "step"; index: number; count: number; doing: string }
  | { kind: "answered"; index: number; question: string }
  | { kind: "ended"; end: RunEnd; message: string };

export function stepsPreview(
  id: number,
  name: string,
  answers: Record<string, string>,
) {
  return invoke<StepsPreview>("steps_preview", { id, name, answers });
}

/**
 * Runs the steps of the snippet `name` on the session `id`, as previewed
 * (`previewed`: the preview's digest; steps that changed since are
 * refused); `confirmed`: the user said yes to the preview (refused
 * otherwise where the preview asks).
 */
export function startSteps(
  id: number,
  name: string,
  answers: Record<string, string>,
  previewed: string,
  confirmed: boolean,
  onevent: (event: RunEvent) => void,
) {
  const onEvent = new Channel<RunEvent>();
  onEvent.onmessage = onevent;
  return invoke<void>("steps_start", {
    id,
    name,
    answers,
    previewed,
    confirmed,
    onEvent,
  });
}

export function stopSteps(id: number) {
  return invoke<boolean>("steps_stop", { id });
}

/**
 * A snippet's steps as plain lines, as the snippets file reads (for
 * previews and the editor, which shows them but does not edit them).
 */
export function stepsText(steps: SnippetSteps): string {
  const lines: string[] = [];
  for (const answer of steps.answers)
    lines.push(`answer ${answer.when} -> ${answer.send || "(Enter)"}`);
  for (const stop of steps.stops) lines.push(`stop at ${stop}`);
  for (const step of steps.steps) {
    const wait =
      step.wait.kind === "prompt"
        ? step.send === null
          ? "wait for the prompt"
          : ""
        : step.wait.kind === "nothing"
          ? "(no wait)"
          : `wait for ${step.wait.pattern}`;
    lines.push([step.send ?? "", wait].filter((part) => part).join("  "));
  }
  return lines.join("\n");
}

/** The first line a snippet's steps send, for the Snippets view's rows. */
export function firstSent(steps: SnippetSteps): string {
  return steps.steps.find((step) => step.send !== null)?.send ?? "";
}
