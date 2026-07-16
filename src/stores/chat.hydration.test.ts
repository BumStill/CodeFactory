// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";

import type { Message } from "../lib/tauri";
import { dbMessagesToUI } from "./chat";

describe("persisted chat hydration", () => {
  it("rebuilds a denied tool card from assistant declarations and replay messages", () => {
    const rows: Message[] = [
      {
        id: "user-1",
        session_id: "session-1",
        role: "user",
        content: "run it",
        created_at: 100,
      },
      {
        id: "assistant-tools",
        session_id: "session-1",
        role: "assistant",
        content: "",
        tool_calls: JSON.stringify([
          {
            id: "call-1",
            type: "function",
            function: {
              name: "bash",
              arguments: JSON.stringify({ command: "printf blocked" }),
            },
          },
        ]),
        created_at: 101,
      },
      {
        id: "session-1:call-1:result",
        session_id: "session-1",
        role: "tool",
        content: JSON.stringify({
          tool_call_id: "call-1",
          content: "Tool call cancelled by hook.",
          status: "denied",
        }),
        created_at: 101,
      },
      {
        id: "assistant-final",
        session_id: "session-1",
        role: "assistant",
        content: "The hook blocked it.",
        created_at: 102,
      },
    ];

    const hydrated = dbMessagesToUI(rows);

    expect(hydrated.map((message) => message.role)).toEqual([
      "user",
      "assistant",
      "assistant",
    ]);
    expect(hydrated[1].toolCalls).toEqual([
      expect.objectContaining({
        id: "call-1",
        name: "bash",
        status: "denied",
        isError: true,
        result: "Tool call cancelled by hook.",
      }),
    ]);
    expect(hydrated[1].toolCalls?.[0].args).toContain("printf blocked");
    expect(hydrated[2].content).toBe("The hook blocked it.");
  });

  it("treats a legacy replay without explicit status as a completed result", () => {
    const rows: Message[] = [
      {
        id: "assistant-tools",
        session_id: "session-1",
        role: "assistant",
        content: "",
        tool_calls: JSON.stringify([
          {
            id: "call-legacy",
            type: "function",
            function: { name: "read_file", arguments: "{}" },
          },
        ]),
        created_at: 200,
      },
      {
        id: "legacy-result",
        session_id: "session-1",
        role: "tool",
        content: JSON.stringify({ tool_call_id: "call-legacy", content: "ok" }),
        created_at: 201,
      },
    ];

    const hydrated = dbMessagesToUI(rows);

    expect(hydrated).toHaveLength(1);
    expect(hydrated[0].toolCalls?.[0]).toEqual(
      expect.objectContaining({ status: "done", result: "ok", isError: false }),
    );
  });

  it("restores cancelled tool calls as cancelled rather than failed", () => {
    const rows: Message[] = [
      {
        id: "assistant-tools",
        session_id: "session-1",
        role: "assistant",
        content: "",
        tool_calls: JSON.stringify([
          {
            id: "call-cancelled",
            type: "function",
            function: { name: "bash", arguments: "{}" },
          },
        ]),
        created_at: 300,
      },
      {
        id: "session-1:call-cancelled:result",
        session_id: "session-1",
        role: "tool",
        content: JSON.stringify({
          tool_call_id: "call-cancelled",
          content: "Tool call cancelled by user.",
          status: "cancelled",
        }),
        created_at: 301,
      },
    ];

    expect(dbMessagesToUI(rows)[0].toolCalls?.[0]).toEqual(
      expect.objectContaining({ status: "cancelled", isError: true }),
    );
  });
});
