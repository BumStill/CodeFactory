// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";

import type { UIMessage } from "../stores/chatEvents";
import { currentTurnOwnership } from "./turnOwnership";

function activity(
  rootTurnId: string,
  objectiveStatus: "active" | "waiting_system",
  terminalReason: string | null = null,
): NonNullable<UIMessage["turnActivity"]> {
  return {
    rootTurnId,
    revision: terminalReason ? 9 : 1,
    phase: terminalReason ? "waiting" : "working",
    status: objectiveStatus,
    kind: terminalReason ?? "tool",
    label: terminalReason ? "系统已登记故障" : "正在执行",
    waitingReason: terminalReason,
    updatedAt: terminalReason ? 9 : 1,
    terminalReason,
    objectiveStatus,
  };
}

describe("currentTurnOwnership", () => {
  it("keeps a steer inside the earlier root and honors terminal dominance", () => {
    const ownership = currentTurnOwnership([
      { id: "root-1", role: "user", content: "完成任务", createdAt: 1 },
      {
        id: "assistant-1",
        role: "assistant",
        rootTurnId: "root-1",
        content: "正在执行",
        createdAt: 2,
        turnActivity: activity("root-1", "waiting_system"),
      },
      { id: "steer-1", role: "user", content: "继续收尾", createdAt: 3 },
      {
        id: "assistant-2",
        role: "assistant",
        rootTurnId: "root-1",
        content: "系统已登记故障",
        createdAt: 4,
        turnSettledAt: 9,
        turnActivity: activity(
          "root-1",
          "waiting_system",
          "technical_recovery_exhausted",
        ),
      },
    ]);

    expect(ownership.rootTurnId).toBe("root-1");
    expect(ownership.messages.map((message) => message.id)).toEqual([
      "root-1",
      "assistant-1",
      "steer-1",
      "assistant-2",
    ]);
    expect(ownership.released).toBe(true);
    expect(ownership.systemHeld).toBe(false);
  });

  it("does not let an older incident release a genuinely active next root", () => {
    const ownership = currentTurnOwnership([
      { id: "root-1", role: "user", content: "旧任务", createdAt: 1 },
      {
        id: "incident-1",
        role: "assistant",
        rootTurnId: "root-1",
        content: "系统已登记故障",
        createdAt: 2,
        turnSettledAt: 2,
        turnActivity: activity(
          "root-1",
          "waiting_system",
          "technical_recovery_exhausted",
        ),
      },
      { id: "root-2", role: "user", content: "新任务", createdAt: 3 },
      {
        id: "assistant-2",
        role: "assistant",
        rootTurnId: "root-2",
        content: "正在执行",
        createdAt: 4,
        turnActivity: activity("root-2", "active"),
      },
    ]);

    expect(ownership.rootTurnId).toBe("root-2");
    expect(ownership.messages.map((message) => message.id)).toEqual([
      "root-2",
      "assistant-2",
    ]);
    expect(ownership.released).toBe(false);
    expect(ownership.systemHeld).toBe(true);
  });

  it("treats a just-submitted unprojected user row as the new root", () => {
    const ownership = currentTurnOwnership([
      { id: "root-1", role: "user", content: "旧任务", createdAt: 1 },
      {
        id: "incident-1",
        role: "assistant",
        rootTurnId: "root-1",
        content: "系统已登记故障",
        createdAt: 2,
        turnSettledAt: 2,
        turnActivity: activity(
          "root-1",
          "waiting_system",
          "technical_recovery_exhausted",
        ),
      },
      { id: "root-2", role: "user", content: "新任务", createdAt: 3 },
      { id: "assistant-2", role: "assistant", content: "", createdAt: 4 },
    ]);

    expect(ownership.rootTurnId).toBe("root-2");
    expect(ownership.messages.map((message) => message.id)).toEqual([
      "root-2",
      "assistant-2",
    ]);
    expect(ownership.released).toBe(false);
  });
});
