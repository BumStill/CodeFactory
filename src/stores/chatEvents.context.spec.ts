// SPDX-License-Identifier: Apache-2.0
/// <reference types="vite/client" />
import { describe, expect, it, vi } from "vitest";
import { reduceChatStreamEvent, type ChatEventState } from "./chatEvents";
import source from "./chatEvents.ts?raw";

function baseState(): ChatEventState {
  return {
    messages: [
      {
        id: "assistant-1",
        role: "assistant",
        content: "",
        toolCalls: [],
        createdAt: 1,
      },
    ],
    streaming: true,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
    contextUsage: null,
    compressionToast: null,
  };
}

describe("chat context stream events", () => {
  it("updates the context usage meter from backend stream events", () => {
    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "context_usage",
        used_tokens: 128_000,
        limit_tokens: 200_000,
        max_limit_tokens: 1_000_000,
      },
      "assistant-1",
    );

    expect(next.contextUsage).toEqual({
      used: 128_000,
      limit: 200_000,
      maxLimit: 1_000_000,
    });
  });

  it("keeps old context events compatible when no expandable limit is present", () => {
    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "context_usage",
        used_tokens: 10_000,
        limit_tokens: 200_000,
      },
      "assistant-1",
    );

    expect(next.contextUsage?.maxLimit).toBe(200_000);
  });

  it("creates a fresh compression toast from backend stream events", () => {
    vi.spyOn(Date, "now").mockReturnValue(42);

    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "context_compressed",
        elided_count: 3,
        tokens_freed: 19_000,
      },
      "assistant-1",
    );

    expect(next.compressionToast).toEqual({
      elidedCount: 3,
      tokensFreed: 19_000,
      id: 42,
    });
  });

  it("records model transport retries on the active assistant message", () => {
    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "transport_retry",
        label: "OpenAI-compatible chat stream request",
        attempt: 1,
        max_attempts: 3,
        delay_ms: 300,
        reason: "HTTP 503 Service Unavailable",
      },
      "assistant-1",
    );

    expect(next.messages[0].transportRetries).toEqual([
      {
        label: "OpenAI-compatible chat stream request",
        attempt: 1,
        maxAttempts: 3,
        delayMs: 300,
        reason: "HTTP 503 Service Unavailable",
      },
    ]);
  });

  it("keeps exactly one reducer branch for each context event type", () => {
    expect(source.match(/case "context_usage"/g)).toHaveLength(1);
    expect(source.match(/case "context_compressed"/g)).toHaveLength(1);
    expect(source.match(/case "transport_retry"/g)).toHaveLength(1);
  });
});
