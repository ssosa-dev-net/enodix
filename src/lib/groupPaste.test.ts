// The multi-send group's pastes (`groupPaste.ts`, #87, #123): planned for
// every member, asked once, and sent to the members as they are when it
// is sent. A group that changes while the question is open gets nothing.

import { beforeEach, describe, expect, test } from "vitest";
import {
  GroupPaste,
  type GroupMember,
  type GroupPasteHooks,
  type GroupPasteQuestion,
  type MemberTerminal,
} from "./groupPaste";
import { fakeShell, pending, settle, type FakeShell } from "../test/shell";

const SAFE = { environment: null, lineDelayMs: 0 };
const PRODUCTION = { environment: "production", lineDelayMs: 50 } as const;

function member(tab: number, session: number, name: string): GroupMember {
  return {
    window: 0,
    tab,
    session,
    name,
    safety: tab === 2 ? PRODUCTION : SAFE,
    host: name,
    connected: true,
  };
}

const MEMBERS = [member(1, 11, "edge-1"), member(2, 12, "core-1")];

let shell: FakeShell;
/** The group's revision as the shell has it. */
let revision: number;

beforeEach(() => {
  shell = fakeShell();
  revision = 4;
  shell.answer("group_members", () => ({ revision, members: MEMBERS }));
  // As the core plans it: a line per line, asked first (a production
  // member is in the group).
  shell.answer("paste_plan_group", (args) => {
    const members = args.members as { text: string; safety: typeof SAFE }[];
    return {
      members: members.map(({ text, safety }) => ({
        lines: text.split(/(?<=\n)/),
        confirm: true,
        lineDelayMs: safety.lineDelayMs,
      })),
      confirm: true,
      environment: "production",
    };
  });
  shell.answer("group_paste_check", (args) => {
    if (args.revision !== revision)
      throw "the multi-send group changed: a tab left";
  });
  shell.answer("group_paste_send", (args) => {
    if (args.revision !== revision)
      throw "the multi-send group changed: a tab left";
  });
});

/** A window's group pastes, with each question kept for the test to
 * answer, and every failure said. */
function pastingWindow() {
  const questions: {
    tab: number;
    question: GroupPasteQuestion;
    answer: (send: boolean) => void;
  }[] = [];
  const failures: unknown[] = [];
  let withdrawn = 0;
  const lines: { session: number; lines: string[] }[] = [];
  const terminal = (session: number): MemberTerminal => ({
    sessionId: () => session,
    pasteLines: async (sent) => void lines.push({ session, lines: sent }),
  });
  const hooks: GroupPasteHooks = {
    terminals: { 1: terminal(11), 2: terminal(12) },
    ready: () => true,
    ask: (tab, question) => {
      const answer = pending<boolean>();
      questions.push({ tab, question, answer: answer.resolve });
      return answer.promise;
    },
    withdraw: () => {
      withdrawn += 1;
      // Closed as Cancel closes it.
      questions.at(-1)?.answer(false);
    },
    failed: (reason) => failures.push(reason),
  };
  return {
    paste: new GroupPaste(hooks),
    questions,
    failures,
    lines,
    withdrawn: () => withdrawn,
  };
}

describe("a group paste", () => {
  test("is asked once, for every member, then sent to each", async () => {
    const { paste, questions } = pastingWindow();
    paste.groupChanged(revision);
    paste.paste(1, "conf t\nend\n");
    await settle();
    expect(questions).toHaveLength(1);
    expect(questions[0].tab).toBe(1);
    expect(questions[0].question.members.map((each) => each.name)).toEqual([
      "edge-1",
      "core-1",
    ]);
    // Nothing went anywhere before the answer.
    expect(shell.called("group_paste_send")).toEqual([]);

    questions[0].answer(true);
    await settle();
    expect(shell.called("group_paste_send").map((call) => call.args)).toEqual([
      {
        tab: 1,
        revision: 4,
        members: [
          { session: 11, lines: ["conf t\n", "end\n"], lineDelayMs: 0 },
          { session: 12, lines: ["conf t\n", "end\n"], lineDelayMs: 50 },
        ],
      },
    ]);
  });

  test("answered Cancel sends nothing", async () => {
    const { paste, questions, failures } = pastingWindow();
    paste.paste(1, "reload\n");
    await settle();
    questions[0].answer(false);
    await settle();
    expect(shell.called("group_paste_send")).toEqual([]);
    expect(failures).toEqual([]);
  });

  test("a group that changes while it is asked sends nothing", async () => {
    const { paste, questions, failures, withdrawn } = pastingWindow();
    paste.groupChanged(revision);
    paste.paste(1, "reload\n");
    await settle();
    expect(questions).toHaveLength(1);

    // A tab leaves the group: this window is shown the newer layout.
    revision = 5;
    paste.groupChanged(revision);
    await settle();
    expect(withdrawn()).toBe(1);
    expect(shell.called("group_paste_send")).toEqual([]);
    // Called off, and the user is told why.
    expect(failures).toEqual(["the multi-send group changed: a tab left"]);
  });

  test("a yes given after the group changed sends nothing", async () => {
    let withdrawals = 0;
    const questions: ((send: boolean) => void)[] = [];
    const failures: unknown[] = [];
    // A window whose question stays open after the change (it answers on
    // its own time).
    const terminal = (session: number): MemberTerminal => ({
      sessionId: () => session,
      pasteLines: async () => {},
    });
    const paste = new GroupPaste({
      terminals: { 1: terminal(11), 2: terminal(12) },
      ready: () => true,
      ask: () => {
        const answer = pending<boolean>();
        questions.push(answer.resolve);
        return answer.promise;
      },
      withdraw: () => (withdrawals += 1),
      failed: (reason) => failures.push(reason),
    });
    paste.paste(1, "reload\n");
    await settle();
    revision = 5;
    paste.groupChanged(revision);
    questions[0](true);
    await settle();
    expect(withdrawals).toBe(1);
    expect(shell.called("group_paste_send")).toEqual([]);
    expect(failures).toHaveLength(1);
  });

  test("a change seen before it is asked: never asked, nothing sent", async () => {
    const { paste, questions, failures } = pastingWindow();
    const planned = pending<unknown>();
    shell.answer("paste_plan_group", () => planned.promise);
    paste.paste(1, "reload\n");
    await settle();
    // The members were read at revision 4; the group changes while the
    // core plans the paste.
    revision = 5;
    paste.groupChanged(revision);
    planned.resolve({
      members: MEMBERS.map(() => ({
        lines: ["reload\n"],
        confirm: true,
        lineDelayMs: 0,
      })),
      confirm: true,
      environment: "production",
    });
    await settle();
    expect(questions).toEqual([]);
    expect(shell.called("group_paste_send")).toEqual([]);
    expect(failures).toHaveLength(1);
  });

  test("a change the window has not seen yet is refused by the shell", async () => {
    const { paste, questions, failures } = pastingWindow();
    paste.paste(1, "reload\n");
    await settle();
    // The shell's group moved on; this window's layout did not yet.
    revision = 5;
    questions[0].answer(true);
    await settle();
    // Sent at revision 4 and refused whole: no member got a line.
    expect(shell.called("group_paste_send")).toHaveLength(1);
    expect(failures).toEqual(["the multi-send group changed: a tab left"]);
  });

  test("pastes go one at a time, in order", async () => {
    const { paste, questions } = pastingWindow();
    paste.paste(1, "first\n");
    paste.paste(1, "second\n");
    await settle();
    // The second is not asked about while the first is.
    expect(questions).toHaveLength(1);
    questions[0].answer(true);
    await settle();
    expect(questions).toHaveLength(2);
    expect(questions[1].question.text).toBe("second\n");
  });

  test("a member's lines from any window go through its own terminal", async () => {
    const { lines, failures } = pastingWindow();
    await settle();
    await shell.emit("group-paste-lines", {
      tab: 2,
      session: 12,
      lines: ["show run\n"],
      lineDelayMs: 50,
    });
    // One that moved to another of this window's tabs is found there.
    await shell.emit("group-paste-lines", {
      tab: 9,
      session: 11,
      lines: ["show clock\n"],
      lineDelayMs: 0,
    });
    // One no longer in this window is said, not sent elsewhere.
    await shell.emit("group-paste-lines", {
      tab: 3,
      session: 13,
      lines: ["reload\n"],
      lineDelayMs: 0,
    });
    await settle();
    expect(lines).toEqual([
      { session: 12, lines: ["show run\n"] },
      { session: 11, lines: ["show clock\n"] },
    ]);
    expect(failures).toHaveLength(1);
  });
});
