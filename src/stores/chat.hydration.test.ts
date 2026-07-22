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

  it("hydrates only the original request and final answer across internal recovery rounds", () => {
    const rows: Message[] = [
      {
        id: "user",
        session_id: "session-1",
        role: "user",
        content: "把拆任务内置到当前 session。",
        created_at: 99,
      },
      {
        id: "early-tool-turn",
        session_id: "session-1",
        role: "assistant",
        content: "先运行与用户无关的内部检查。",
        tool_calls: JSON.stringify([
          {
            id: "early-probe",
            type: "function",
            function: { name: "bash", arguments: "{}" },
          },
        ]),
        created_at: 99.5,
      },
      {
        id: "early-tool-result",
        session_id: "session-1",
        role: "tool",
        content: JSON.stringify({
          tool_call_id: "early-probe",
          content: "internal check passed",
          status: "done",
        }),
        created_at: 99.6,
      },
      {
        id: "candidate",
        session_id: "session-1",
        role: "assistant",
        content: "unrelated candidate answer",
        created_at: 100,
        completion_state: "rejected_candidate",
      },
      {
        id: "gate-nudge",
        session_id: "session-1",
        role: "user",
        content: "The completion gate rejected the attempted final response…",
        created_at: 101,
        completion_state: "gate_recovery",
      },
      {
        id: "internal-tool-turn",
        session_id: "session-1",
        role: "assistant",
        content: "后台服务已运行，现在执行后续探针。",
        tool_calls: JSON.stringify([
          {
            id: "probe-1",
            type: "function",
            function: { name: "bash", arguments: "{}" },
          },
        ]),
        created_at: 102,
      },
      {
        id: "internal-tool-result",
        session_id: "session-1",
        role: "tool",
        content: JSON.stringify({
          tool_call_id: "probe-1",
          content: "later client probe passed",
          status: "done",
        }),
        created_at: 103,
      },
      {
        id: "gate-ready",
        session_id: "session-1",
        role: "user",
        content: "The structured completion evidence is satisfied…",
        created_at: 104,
        completion_state: "gate_ready",
      },
      {
        id: "final",
        session_id: "session-1",
        role: "assistant",
        content: "已完成：拆任务已内置到当前会话。",
        created_at: 105,
      },
    ];

    const hydrated = dbMessagesToUI(rows);
    expect(hydrated.map(({ role, content }) => ({ role, content }))).toEqual([
      { role: "user", content: "把拆任务内置到当前 session。" },
      { role: "assistant", content: "已完成：拆任务已内置到当前会话。" },
    ]);
    expect(hydrated.flatMap((message) => message.toolCalls ?? [])).toEqual([]);
  });

  it("hydrates an internal recovery failure without raw control-loop details", () => {
    const rows: Message[] = [
      {
        id: "user",
        session_id: "session-1",
        role: "user",
        content: "完成这个修改。",
        created_at: 1,
      },
      {
        id: "candidate",
        session_id: "session-1",
        role: "assistant",
        content: "draft",
        completion_state: "rejected_candidate",
        created_at: 2,
      },
      {
        id: "recovery",
        session_id: "session-1",
        role: "user",
        content: "The completion gate rejected the response",
        completion_state: "gate_recovery",
        created_at: 3,
      },
      {
        id: "error",
        session_id: "session-1",
        role: "user",
        content: "回合中断:Completion blocked: unresolved probe fingerprint",
        completion_state: "turn_error",
        created_at: 4,
      },
    ];

    const hydrated = dbMessagesToUI(rows);
    expect(hydrated.map(({ role, content, completionState }) => ({
      role,
      content,
      completionState,
    }))).toEqual([
      { role: "user", content: "完成这个修改。", completionState: undefined },
      {
        role: "user",
        content: "本次处理未能完成，请重试。",
        completionState: "turn_error",
      },
    ]);
  });

});
