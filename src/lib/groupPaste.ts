// The multi-send group's pastes across windows (#87). Typed keys fan out
// in the shell; a paste (or a group snippet) is planned in the window it
// was pasted in, for every member wherever it is, and confirmed once
// there. Then each member's lines go through that member's own terminal
// (its bracketed paste, its line delay, stopping when its session ends or
// it leaves the group), through the shell (`group_paste_send` →
// `group-paste-lines`) to the window showing it, this one included.
// Nothing is sent twice, and nothing goes through the fan-out.
//
// One rule in every window (#123): a group paste goes to the members as
// they are when it is sent. The members come with the group's revision;
// the question shows them, and closes when the group changes (a tab joins
// or leaves, a member closes, the group ends), the paste called off and
// the reason said. The shell refuses the same at sending, so a change
// this window has not seen yet calls it off too: never sent to a tab that
// has left.

import { invoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Layout, TabId } from "./layout";
import { planGroupPaste } from "./paste";
import type { PasteMember } from "./PasteConfirm.svelte";
import type { Environment, PasteSafety } from "./workspace";
import { expandSnippetForGroup } from "./workspace";
import { MAIN_WINDOW_ID, thisSessionWindow } from "./windows";

/** A member of the group as the pasting window sees it, from the app's
 * record of its session, whichever window shows it. */
export interface GroupMember {
  window: number;
  tab: TabId;
  session: number;
  name: string;
  /** What applies to its pastes (its host's as the workspace sets it
   * now, else as it opened). */
  safety: PasteSafety;
  /** Its host in the open workspace, for snippet values. */
  host: string | null;
  connected: boolean;
}

/** Who a paste goes to, and the group's revision they were read at. */
export interface GroupMembers {
  revision: number;
  members: GroupMember[];
}

/** Where a paste in this window's tab `tab` goes: every member, the one
 * pasted in first. Refused, with the reason, when `tab` is no longer a
 * member (the paste is called off). */
export function groupMembers(tab: TabId) {
  return invoke<GroupMembers>("group_members", { tab });
}

/** What applies to the pastes of the session `id`, as for a member. */
export function sessionSafety(id: number) {
  return invoke<PasteSafety>("session_safety", { id });
}

/** A member's lines of a confirmed group paste, sent to its window. */
export interface GroupPasteLines {
  tab: TabId;
  session: number;
  lines: string[];
  lineDelayMs: number;
}

/** Calls `handler` with each member's lines another window sends to one of
 * this window's terminals. */
export function onGroupPasteLines(
  handler: (lines: GroupPasteLines) => void,
): Promise<UnlistenFn> {
  return getCurrentWebviewWindow().listen<GroupPasteLines>(
    "group-paste-lines",
    (event) => handler(event.payload),
  );
}

/** Calls `handler` with this window's layout whenever the group changed
 * in any window. */
export function onLayoutChanged(
  handler: (layout: Layout) => void,
): Promise<UnlistenFn> {
  return getCurrentWebviewWindow().listen<Layout>("layout-changed", (event) =>
    handler(event.payload),
  );
}

/** The question asked once for a group paste, over the tab pasted in. */
export interface GroupPasteQuestion {
  text: string;
  lineCount: number;
  environment: Environment | null;
  members: PasteMember[];
}

/** A window's terminal, as a member's lines go through it. */
export interface MemberTerminal {
  sessionId(): number | null;
  pasteLines(lines: string[], lineDelayMs: number): Promise<void>;
}

/** What a window lends the group's pastes. */
export interface GroupPasteHooks {
  /** This window's terminals, by tab (kept up to date by the window). */
  terminals: Record<TabId, MemberTerminal | null>;
  /** Whether this window's tab can take a paste now (its session is
   * connected). */
  ready: (tab: TabId) => boolean;
  /** Asks `question` over `tab`; resolves to whether to send. */
  ask: (tab: TabId, question: GroupPasteQuestion) => Promise<boolean>;
  /** Closes the question `ask` shows, answering it as Cancel does: the
   * group changed while it was open. */
  withdraw: () => void;
  failed: (reason: unknown) => void;
}

/**
 * A window's group pastes, one at a time, in order; and the lines other
 * windows send to its members, each through that member's terminal.
 */
export class GroupPaste {
  #chain: Promise<void> = Promise.resolve();
  /** This window's id in the core. */
  readonly window = thisSessionWindow() ?? MAIN_WINDOW_ID;

  constructor(private hooks: GroupPasteHooks) {
    void onGroupPasteLines((sent) => this.#receive(sent));
  }

  /** This window's terminal showing `session`: in `tab`, or another tab
   * of this window it has moved to. */
  #terminal(tab: TabId, session: number): MemberTerminal | null {
    const terminals = this.hooks.terminals;
    if (terminals[tab]?.sessionId() === session) return terminals[tab];
    return (
      Object.values(terminals).find(
        (terminal) => terminal?.sessionId() === session,
      ) ?? null
    );
  }

  /** A member's lines of a paste confirmed in any window (this one too). */
  #receive({ tab, session, lines, lineDelayMs }: GroupPasteLines) {
    const terminal = this.#terminal(tab, session);
    if (!terminal) {
      this.hooks.failed(
        "a member moved away as the paste arrived and did not get it",
      );
      return;
    }
    terminal.pasteLines(lines, lineDelayMs).catch(this.hooks.failed);
  }

  /** The group's revision in this window's newest layout. */
  #seen = 0;
  /** The question open for a group paste: the revision it was planned at,
   * and whether the group changed since. */
  #asking: { revision: number; changed: boolean } | null = null;

  /**
   * The group's revision this window's layout shows now (the window calls
   * this whenever it shows a newer layout): a question asked at an older
   * one is out of date, so it closes, and its paste is called off (#123).
   */
  groupChanged(revision: number) {
    if (revision <= this.#seen) return;
    this.#seen = revision;
    const asking = this.#asking;
    if (asking && revision > asking.revision && !asking.changed) {
      asking.changed = true;
      this.hooks.withdraw();
    }
  }

  /** The members that take a paste from `from` now: connected ones (in
   * this window, with a terminal), the one pasted in first; and the
   * group's revision. Refused when `from` is no longer a member. */
  async receiving(from: TabId): Promise<GroupMembers> {
    const { revision, members } = await groupMembers(from);
    return {
      revision,
      members: members.filter(
        (member) =>
          member.connected &&
          (member.window !== this.window ||
            (this.hooks.ready(member.tab) &&
              !!this.#terminal(member.tab, member.session))),
      ),
    };
  }

  /** Pastes `text` into every member, as pasted in `from`. */
  paste(from: TabId, text: string) {
    this.#queue(async () =>
      this.#send(from, await this.receiving(from), () => text),
    );
  }

  /** Pastes each member's own text (`texts`, by session: a snippet filled
   * per member) into `receiving`'s members, as sent from `from`. */
  pasteEach(from: TabId, receiving: GroupMembers, texts: Map<number, string>) {
    this.#queue(() =>
      this.#send(from, receiving, (member) => texts.get(member.session)),
    );
  }

  #queue(send: () => Promise<void>) {
    this.#chain = this.#chain.then(send).catch(this.hooks.failed);
  }

  async #send(
    from: TabId,
    { revision, members: receiving }: GroupMembers,
    textOf: (member: GroupMember) => string | undefined,
  ) {
    const members = receiving.filter((member) => textOf(member) !== undefined);
    if (members.length === 0) return;
    const plan = await planGroupPaste(
      members.map((member) => ({
        text: textOf(member) ?? "",
        safety: member.safety,
      })),
    );
    if (plan.confirm) {
      // The text pasted in; a member filled differently shows its own.
      const shown = plan.members[0].lines.join("");
      const asking = { revision, changed: this.#seen > revision };
      this.#asking = asking;
      let send = false;
      try {
        if (!asking.changed) {
          send = await this.hooks.ask(from, {
            text: shown,
            lineCount: plan.members[0].lines.length,
            environment: plan.environment,
            members: members.map((member, index) => {
              const text = plan.members[index].lines.join("");
              return {
                name: member.name,
                environment: member.safety.environment,
                lineDelayMs: plan.members[index].lineDelayMs,
                text: text === shown ? null : text,
              };
            }),
          });
        }
      } finally {
        this.#asking = null;
      }
      if (asking.changed) {
        // The members it showed are not the group's any more: called off,
        // and why (the shell's words).
        await invoke<void>("group_paste_check", { tab: from, revision });
        throw "the multi-send group changed while the paste was asked about, so nothing was pasted";
      }
      if (!send) return;
    }
    // To the members as they are now: refused whole (nothing sent, the
    // reason said) if the group changed since they were read.
    await invoke<void>("group_paste_send", {
      tab: from,
      revision,
      members: members.map((member, index) => ({
        session: member.session,
        lines: plan.members[index].lines,
        lineDelayMs: plan.members[index].lineDelayMs,
      })),
    });
  }
}

/** The snippet `name` filled for each of `members` from its own host and
 * `answers`: each member's text by session, or the variables to ask. */
export async function expandForMembers(
  name: string,
  members: GroupMember[],
  answers: Record<string, string>,
): Promise<
  | { kind: "missing"; names: string[] }
  | { kind: "texts"; texts: Map<number, string> }
> {
  const expansion = await expandSnippetForGroup(
    name,
    members.map((member) => member.host),
    answers,
  );
  if (expansion.kind === "missing")
    return { kind: "missing", names: expansion.names };
  const filled = expansion.members;
  return {
    kind: "texts",
    texts: new Map(
      members.map((member, index) => [
        member.session,
        filled[index].map((line) => `${line}\n`).join(""),
      ]),
    ),
  };
}
