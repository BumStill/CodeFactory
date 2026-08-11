// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";

import type {
  Message,
  TurnActivitySnapshot,
  TurnPlanSnapshot,
} from "../lib/tauri";
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

  it("hydrates every step of a recovered turn, dropping only the gate's own prompts", () => {
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
      { role: "assistant", content: "先运行与用户无关的内部检查。" },
      { role: "assistant", content: "unrelated candidate answer" },
      { role: "assistant", content: "后台服务已运行，现在执行后续探针。" },
      { role: "assistant", content: "已完成：拆任务已内置到当前会话。" },
    ]);
    // Both tool cards survive with their replayed results attached.
    expect(
      hydrated.flatMap((message) => message.toolCalls ?? []).map((tc) => [tc.id, tc.result]),
    ).toEqual([
      ["early-probe", "internal check passed"],
      ["probe-1", "later client probe passed"],
    ]);
    // The gate's injected prompts are the only thing withheld.
    expect(JSON.stringify(hydrated)).not.toMatch(/completion gate|completion evidence/i);
  });

  it("keeps the draft and the raw turn error when a recovery round dies", () => {
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
      { role: "assistant", content: "draft", completionState: "rejected_candidate" },
      {
        role: "user",
        content: "回合中断:Completion blocked: unresolved probe fingerprint",
        completionState: "turn_error",
      },
    ]);
  });

  it("shows an interrupted recovery round as ordinary work, never the gate's prompt", () => {
    const rows: Message[] = [
      {
        id: "user",
        session_id: "session-1",
        role: "user",
        content: "完成这个修改。",
        created_at: 1,
      },
      {
        id: "recovery",
        session_id: "session-1",
        role: "user",
        content: "The completion gate rejected the response: secret blocker details",
        completion_state: "gate_recovery",
        created_at: 2,
      },
      {
        id: "internal-tool-turn",
        session_id: "session-1",
        role: "assistant",
        content: "",
        tool_calls: JSON.stringify([
          {
            id: "probe-1",
            type: "function",
            function: {
              name: "bash",
              arguments: "{\"command\":\"curl https://secret.example\"}",
            },
          },
        ]),
        created_at: 3,
      },
    ];

    const hydrated = dbMessagesToUI(rows);
    expect(hydrated).toHaveLength(2);
    // The recovery round's tool call is work the agent actually did — it
    // renders like any other tool card, args included.
    expect(hydrated[1]).toEqual(
      expect.objectContaining({
        role: "assistant",
        content: "",
        toolCalls: [expect.objectContaining({ id: "probe-1", name: "bash" })],
      }),
    );
    // The injected gate prompt is the one thing that stays out of the transcript.
    expect(JSON.stringify(hydrated)).not.toMatch(/secret blocker|completion gate/i);
  });

  it("attaches a persisted verification warning to the final answer", () => {
    const rows: Message[] = [
      {
        id: "user",
        session_id: "session-1",
        role: "user",
        content: "完成并验证。",
        created_at: 1,
      },
      {
        id: "answer",
        session_id: "session-1",
        role: "assistant",
        content: "已完成，但一项端到端验证不可用。",
        created_at: 2,
      },
      {
        id: "warning",
        session_id: "session-1",
        role: "user",
        content: "⚠ 以上回复未经完整验证：端到端环境不可用。",
        completion_state: "gate_warning",
        created_at: 3,
      },
    ];

    const hydrated = dbMessagesToUI(rows);
    expect(hydrated).toHaveLength(2);
    expect(hydrated[1].content).toBe("已完成，但一项端到端验证不可用。");
    expect(hydrated[1].gateActions).toEqual([
      {
        kind: "warning",
        detail: "⚠ 以上回复未经完整验证：端到端环境不可用。",
      },
    ]);
  });

  it("restores the latest plan and bounded turn evidence on the final answer", () => {
    const rows: Message[] = [
      {
        id: "root-turn",
        session_id: "session-1",
        role: "user",
        content: "完成并验证。",
        created_at: 1,
      },
      {
        id: "tool-round",
        session_id: "session-1",
        role: "assistant",
        content: "开始验证。",
        tool_calls: JSON.stringify([
          {
            id: "plan-call",
            type: "function",
            function: { name: "update_plan", arguments: "{}" },
          },
          {
            id: "build-call",
            type: "function",
            function: {
              name: "bash",
              arguments: JSON.stringify({ command: "pnpm build" }),
            },
          },
        ]),
        created_at: 2,
      },
      {
        id: "build-result",
        session_id: "session-1",
        role: "tool",
        content: JSON.stringify({
          tool_call_id: "build-call",
          content: "ok",
          status: "done",
        }),
        created_at: 3,
      },
      {
        id: "final",
        session_id: "session-1",
        role: "assistant",
        content: "已完成。",
        created_at: 4,
      },
    ];
    const plans: TurnPlanSnapshot[] = [
      {
        root_turn_id: "root-turn",
        revision: 3,
        steps: [
          {
            id: "implement",
            title: "实现",
            kind: "implementation",
            status: "completed",
          },
          {
            id: "verify",
            title: "验证",
            kind: "verification",
            status: "completed",
          },
        ],
        created_at: 3,
      },
    ];

    const hydrated = dbMessagesToUI(rows, plans);
    expect(hydrated[1].toolCalls?.map((tool) => tool.name)).toEqual(["bash"]);
    expect(hydrated[2].plan).toEqual(
      expect.objectContaining({ rootTurnId: "root-turn", revision: 3 }),
    );
    expect(hydrated[2].turnToolCalls).toEqual([
      expect.objectContaining({ id: "build-call", status: "done" }),
    ]);
    expect(hydrated[2].turnToolCallCount).toBe(1);
    expect(hydrated[2].durationMs).toBe(3);
  });

  it("restores persisted activity onto the owning turn", () => {
    const rows: Message[] = [
      {
        id: "root-turn",
        session_id: "session-1",
        role: "user",
        content: "修复并验证",
        created_at: 1,
      },
      {
        id: "assistant",
        session_id: "session-1",
        role: "assistant",
        content: "当前被外部条件阻断。",
        created_at: 2,
      },
    ];
    const states: TurnActivitySnapshot[] = [
      {
        root_turn_id: "root-turn",
        revision: 7,
        phase: "finalizing",
        status: "blocked",
        recent_activity_kind: "blocked",
        recent_activity_label: "任务已在明确边界停止",
        waiting_reason: null,
        updated_at: 3,
        terminal_reason: "tool_blocked",
        objective_id: "objective-1",
        objective_status: "waiting_system",
        recovery_owner: "objective-supervisor",
        next_observation_at: 42_000,
        last_progress_at: 41_000,
      },
    ];

    const hydrated = dbMessagesToUI(rows, [], states);

    expect(hydrated[1].turnActivity).toEqual(
      expect.objectContaining({
        rootTurnId: "root-turn",
        revision: 7,
        status: "blocked",
        terminalReason: "tool_blocked",
        objectiveId: "objective-1",
        objectiveStatus: "waiting_system",
        recoveryOwner: "objective-supervisor",
        nextObservationAt: 42_000,
        lastProgressAt: 41_000,
      }),
    );
  });

  it("keeps a hydrated system-owned objective when no assistant row exists yet", () => {
    const rows: Message[] = [
      {
        id: "root-before-crash",
        session_id: "session-1",
        role: "user",
        content: "完成并发布",
        created_at: 1,
      },
    ];
    const states: TurnActivitySnapshot[] = [
      {
        root_turn_id: "root-before-crash",
        revision: 2,
        phase: "recovering",
        status: "active",
        recent_activity_kind: "remediation",
        recent_activity_label: "正在恢复模型连接",
        waiting_reason: "等待退避窗口结束",
        updated_at: 3,
        terminal_reason: null,
        objective_id: "objective-before-crash",
        objective_status: "waiting_system",
        recovery_owner: "objective-supervisor",
        next_observation_at: 42_000,
        last_progress_at: 41_000,
      },
    ];

    const hydrated = dbMessagesToUI(rows, [], states);

    expect(hydrated).toHaveLength(1);
    expect(hydrated[0].role).toBe("user");
    expect(hydrated[0].turnActivity).toEqual(
      expect.objectContaining({
        objectiveId: "objective-before-crash",
        objectiveStatus: "waiting_system",
        recoveryOwner: "objective-supervisor",
        nextObservationAt: 42_000,
        lastProgressAt: 41_000,
      }),
    );
  });

});
