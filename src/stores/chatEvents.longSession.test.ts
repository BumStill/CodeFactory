// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { reduceChatStreamEvent, type ChatEventState, type UIMessage } from "./chatEvents";

describe("long-session stream updates", () => {
  it("updates the active tail without scanning thousands of historical ids", () => {
    let idReads = 0;
    const messages: UIMessage[] = Array.from({ length: 3743 }, (_, index) => {
      const message = {
        role: index % 2 === 0 ? "user" : "assistant",
        content: `history-${index}`,
        createdAt: index,
      } as UIMessage;
      Object.defineProperty(message, "id", {
        enumerable: true,
        configurable: true,
        get() {
          idReads += 1;
          return index === 3742 ? "active-tail" : `history-${index}`;
        },
      });
      return message;
    });
    const state: ChatEventState = {
      messages,
      streaming: true,
      inputTokenTotal: 0,
      outputTokenTotal: 0,
      pendingPermission: null,
      contextUsage: null,
      compressionToast: null,
    };

    const next = reduceChatStreamEvent(
      state,
      { type: "text_delta", content: " delta" },
      "active-tail",
    );

    expect(idReads).toBeLessThanOrEqual(4);
    expect(next.messages).not.toBe(messages);
    for (let index = 0; index < messages.length - 1; index += 1) {
      expect(next.messages[index]).toBe(messages[index]);
    }
    const tailIndex = messages.length - 1;
    expect(next.messages[tailIndex]).not.toBe(messages[tailIndex]);
    expect(next.messages[tailIndex]?.content).toBe("history-3742 delta");
    expect(state.messages[tailIndex]?.content).toBe("history-3742");
  });
});
