// SPDX-License-Identifier: Apache-2.0
//
// CF-HDE-R7 / R9 / R11: one truthful source for "is this session actually
// running, what objective is it in, and what did it last reply".
//
// These cases encode the three real-machine defects the second phase exists to
// close: M56 (a settled turn's leftover `streaming` flag swallowed a new
// message), M53 (the objective kept reading "waiting to retry" while the model
// was actively being called) and M55 (status could not summarise the last
// reply).

import { describe, it, expect } from "vitest";

import { sessionTurnState } from "./sessionTurnState";
import type { UIMessage } from "../stores/chatEvents";

function userMessage(id: string, content = "hi"): UIMessage {
  return { id, role: "user", content, createdAt: 1 };
}

/** 一份完整的活动投影：测试只关心 objective 相关的字段，其余取协议默认值。 */
function activity(overrides: Partial<NonNullable<UIMessage["turnActivity"]>>): NonNullable<UIMessage["turnActivity"]> {
  return {
    rootTurnId: "t1",
    revision: 1,
    phase: "working",
    status: "active",
    kind: "tool",
    label: "",
    waitingReason: null,
    updatedAt: 0,
    terminalReason: null,
    nextObservationAt: null,
    ...overrides,
  };
}

function assistantMessage(
  id: string,
  content: string,
  turnActivity?: Partial<NonNullable<UIMessage["turnActivity"]>>,
): UIMessage {
  return {
    id,
    role: "assistant",
    content,
    createdAt: 2,
    ...(turnActivity ? { turnActivity: activity(turnActivity) } : {}),
  };
}

describe("sessionTurnState", () => {
  it("treats an idle session as not running and reports idle", () => {
    const state = sessionTurnState({
      streaming: false,
      messages: [userMessage("t1"), assistantMessage("a1", "done")],
    });
    expect(state.running).toBe(false);
    expect(state.objectiveState).toBe("idle");
  });

  it("M53: a live model call reads active even if an older wait is on screen", () => {
    const state = sessionTurnState({
      streaming: true,
      // The supervisor already advanced to a live call, but an earlier
      // waiting_system projection is still the last frozen activity.
      messages: [
        userMessage("t1"),
        assistantMessage("a1", "", { objectiveStatus: "waiting_system", revision: 1 }),
      ],
    });
    expect(state.running).toBe(true);
    expect(state.objectiveState).toBe("active");
  });

  it("M56: a settled turn's leftover streaming flag does not count as running", () => {
    const state = sessionTurnState({
      // The runtime still says streaming (the residue), but the turn settled
      // with a terminal objective.
      streaming: true,
      messages: [
        userMessage("t1"),
        assistantMessage("a1", "stopped", {
          objectiveStatus: "failed",
          terminalReason: "chat_identity_unreconcilable",
          revision: 9,
        }),
      ],
    });
    expect(state.running).toBe(false);
    expect(state.objectiveState).toBe("failed");
  });

  it("M56: an authoritative server run flag still counts as running", () => {
    const state = sessionTurnState({
      streaming: false,
      sessionIsRunning: true,
      messages: [userMessage("t1")],
    });
    expect(state.running).toBe(true);
    expect(state.objectiveState).toBe("active");
  });

  it("reports waiting_authorization when a prompt is unresolved", () => {
    const state = sessionTurnState({
      streaming: true,
      waitingPermission: true,
      messages: [userMessage("t1"), assistantMessage("a1", "", { objectiveStatus: "active", revision: 2 })],
    });
    expect(state.objectiveState).toBe("waiting_authorization");
  });

  it("keeps a system-held turn reading waiting_system only when nothing is executing", () => {
    const state = sessionTurnState({
      streaming: false,
      messages: [
        userMessage("t1"),
        assistantMessage("a1", "", { objectiveStatus: "waiting_system", revision: 1 }),
      ],
    });
    expect(state.running).toBe(false);
    expect(state.objectiveState).toBe("waiting_system");
  });

  it("R9: summarises the latest non-empty assistant reply", () => {
    const long = "x".repeat(400);
    const state = sessionTurnState({
      streaming: false,
      messages: [
        userMessage("t1"),
        assistantMessage("a1", "first reply"),
        userMessage("t2"),
        assistantMessage("a2", long),
      ],
    });
    expect(state.latestReply).not.toBeNull();
    expect(state.latestReply!.startsWith("x")).toBe(true);
    expect(state.latestReply!.length).toBeLessThan(long.length);
    expect(state.latestReply!.endsWith("…")).toBe(true);
  });

  it("R9: returns null latest reply when the assistant has said nothing", () => {
    const state = sessionTurnState({
      streaming: true,
      messages: [userMessage("t1"), assistantMessage("a1", "   ")],
    });
    expect(state.latestReply).toBeNull();
  });

  it("R9: passes through the last model-call context size and PR number", () => {
    const state = sessionTurnState({
      streaming: false,
      messages: [userMessage("t1")],
      contextTokens: 12345,
      prNumber: 601,
    });
    expect(state.contextTokens).toBe(12345);
    expect(state.prNumber).toBe(601);
  });
});
