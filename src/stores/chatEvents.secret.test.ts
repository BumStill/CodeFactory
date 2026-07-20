// SPDX-License-Identifier: Apache-2.0
//
// Secure-secret prompt stream events. The secret VALUE never passes through
// this reducer — only the request metadata does; the value goes straight from
// the modal to the provide_secret tauri command.

import { describe, it, expect } from "vitest";
import {
  reduceChatStreamEvent,
  markSecretResponse,
  type ChatEventState,
} from "./chatEvents";

function baseState(): ChatEventState {
  return {
    messages: [
      { id: "assistant-1", role: "assistant", content: "", toolCalls: [], createdAt: 1 },
    ],
    streaming: true,
    inputTokenTotal: 0,
    outputTokenTotal: 0,
    pendingPermission: null,
    contextUsage: null,
    compressionToast: null,
  };
}

describe("secure secret prompt events", () => {
  it("opens a pending secret prompt from a secret_request event", () => {
    const next = reduceChatStreamEvent(
      baseState(),
      {
        type: "secret_request",
        request_id: "req-1",
        purpose: "为 BumStill/CodeFactory 配置 GitHub 访问令牌(repo 权限)",
        hint: "github.com → Settings → Developer settings",
      },
      "assistant-1",
    );
    expect(next.pendingSecret).toEqual({
      requestId: "req-1",
      purpose: "为 BumStill/CodeFactory 配置 GitHub 访问令牌(repo 权限)",
      hint: "github.com → Settings → Developer settings",
    });
    expect(next.streaming).toBe(true);
  });

  it("clears the prompt once answered", () => {
    const open = reduceChatStreamEvent(
      baseState(),
      { type: "secret_request", request_id: "req-2", purpose: "p", hint: "h" },
      "assistant-1",
    );
    const done = markSecretResponse(open);
    expect(done.pendingSecret).toBeNull();
  });
});
