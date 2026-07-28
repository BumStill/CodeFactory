// SPDX-License-Identifier: Apache-2.0
//
// Mid-run steering: a message typed while the agent works reaches the model at
// its next round boundary. Until the loop confirms that, the bubble must read
// as undelivered — the model genuinely has not seen it yet.

import { describe, it, expect } from "vitest";
import { reduceChatStreamEvent, type ChatEventState, type UIMessage } from "./chatEvents";

function stateWith(messages: UIMessage[]): ChatEventState {
  return {
    messages,
    streaming: true,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
    contextUsage: null,
    compressionToast: null,
  };
}

const pendingSteer = (id: string, content: string): UIMessage => ({
  id,
  role: "user",
  content,
  createdAt: 1,
  steerPending: true,
});

describe("steer_applied", () => {
  it("confirms the pending steer and adopts its persisted id", () => {
    const next = reduceChatStreamEvent(
      stateWith([pendingSteer("local-1", "改用 chrome channel")]),
      { type: "steer_applied", message_id: "db-42", content: "改用 chrome channel" },
      "assistant-1",
    );

    expect(next.messages[0].steerPending).toBeUndefined();
    expect(next.messages[0].id).toBe("db-42");
    expect(next.messages[0].content).toBe("改用 chrome channel");
  });

  it("keeps the local id when the run is anonymous and nothing was persisted", () => {
    const next = reduceChatStreamEvent(
      stateWith([pendingSteer("local-1", "别提交")]),
      { type: "steer_applied", message_id: null, content: "别提交" },
      "assistant-1",
    );

    expect(next.messages[0].steerPending).toBeUndefined();
    expect(next.messages[0].id).toBe("local-1");
  });

  it("confirms one steer per event when the same text was sent twice", () => {
    const next = reduceChatStreamEvent(
      stateWith([pendingSteer("local-1", "停"), pendingSteer("local-2", "停")]),
      { type: "steer_applied", message_id: "db-1", content: "停" },
      "assistant-1",
    );

    expect(next.messages[0].steerPending).toBeUndefined();
    expect(next.messages[1].steerPending).toBe(true);
  });

  it("leaves already-delivered user turns alone", () => {
    const delivered: UIMessage = { id: "u1", role: "user", content: "停", createdAt: 1 };
    const next = reduceChatStreamEvent(
      stateWith([delivered, pendingSteer("local-1", "停")]),
      { type: "steer_applied", message_id: "db-1", content: "停" },
      "assistant-1",
    );

    expect(next.messages[0].id).toBe("u1");
    expect(next.messages[1].id).toBe("db-1");
    expect(next.messages[1].steerPending).toBeUndefined();
  });
});
