// SPDX-License-Identifier: Apache-2.0

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { UIMessage } from "../stores/chat";
import { MessageList } from "./MessageList";

/// CF-RSB-R3: the banner shows the real state. These fixtures are synthetic.
function activity(
  overrides: Partial<NonNullable<UIMessage["turnActivity"]>>,
): NonNullable<UIMessage["turnActivity"]> {
  return {
    rootTurnId: "user",
    revision: 3,
    phase: "working",
    status: "active",
    kind: "tool",
    label: "",
    waitingReason: null,
    updatedAt: Date.now(),
    terminalReason: null,
    ...overrides,
  };
}

function turn(activityOverride: Partial<NonNullable<UIMessage["turnActivity"]>>): UIMessage[] {
  return [
    { id: "user", role: "user", content: "把这件事做完", createdAt: Date.now() - 25_000 },
    {
      id: "assistant",
      role: "assistant",
      content: "",
      createdAt: Date.now() - 24_000,
      turnActivity: activity(activityOverride),
    },
  ];
}

describe("session status banner speaks plain language (CF-RSB-R1..R4)", () => {
  it("shows plain running text while the system actually works", () => {
    render(<MessageList messages={turn({ objectiveStatus: "active", label: "正在执行命令" })} streaming cwd={null} />);

    const banner = screen.getByTestId("turn-activity-progress");
    expect(banner).toHaveTextContent("正在执行命令");
    expect(banner).toHaveAttribute("data-status-tone", "progress");
    expect(banner).not.toHaveTextContent("objective-supervisor");
    expect(banner).not.toHaveTextContent("恢复");
    expect(banner).not.toHaveTextContent("观察");
  });

  it("shows waiting with a truthful estimate while waiting to retry", () => {
    render(
      <MessageList
        messages={turn({ objectiveStatus: "waiting_system", nextObservationAt: Date.now() + 12_000 })}
        streaming={false}
        cwd={null}
      />,
    );

    const banner = screen.getByTestId("turn-activity-progress");
    expect(banner).toHaveTextContent("等待");
    expect(banner).toHaveTextContent("约 12 秒后重试");
  });

  it("says 马上重试 instead of the meaningless 0ms for an overdue retry", () => {
    render(
      <MessageList
        messages={turn({
          objectiveStatus: "waiting_system",
          label: "正在执行命令",
          recoveryOwner: "objective-supervisor:chat",
          nextObservationAt: Date.now() - 5_000,
        })}
        streaming={false}
        cwd={null}
      />,
    );

    const banner = screen.getByTestId("turn-activity-progress");
    expect(banner).toHaveTextContent("马上重试");
    expect(banner).not.toHaveTextContent("0ms");
    expect(banner).not.toHaveTextContent("objective-supervisor");
    expect(banner).not.toHaveTextContent("下次观察");
  });

  it("shows no time hint at all when the next attempt is unknown", () => {
    render(
      <MessageList
        messages={turn({ objectiveStatus: "waiting_system", nextObservationAt: null })}
        streaming={false}
        cwd={null}
      />,
    );

    const banner = screen.getByTestId("turn-activity-progress");
    expect(banner).toHaveTextContent("等待");
    expect(banner).not.toHaveTextContent("后重试");
    expect(banner).not.toHaveTextContent("ms");
  });

  it.each(["completed", "failed", "cancelled"] as const)(
    "shows no still-processing banner once the task ended as %s",
    (objectiveStatus) => {
      render(
        <MessageList
          messages={turn({ objectiveStatus, label: "系统仍在处理" })}
          streaming={false}
          cwd={null}
        />,
      );

      expect(screen.queryByTestId("turn-activity-progress")).not.toBeInTheDocument();
    },
  );

  it("tells the user plainly what to do when they must authorize", () => {
    render(
      <MessageList
        messages={turn({ objectiveStatus: "waiting_authorization", waitingReason: "authorization_required" })}
        streaming={false}
        cwd={null}
      />,
    );

    const banner = screen.getByTestId("turn-activity-progress");
    expect(banner).toHaveAttribute("data-status-tone", "warning");
    expect(banner).toHaveTextContent("需要你先授权才能继续");
    expect(banner).not.toHaveTextContent("authorization_required");
  });

  it("tells the user plainly what to decide when a business decision is needed", () => {
    render(
      <MessageList
        messages={turn({ objectiveStatus: "waiting_business_decision", waitingReason: "needs_business_decision" })}
        streaming={false}
        cwd={null}
      />,
    );

    const banner = screen.getByTestId("turn-activity-progress");
    expect(banner).toHaveTextContent("需要你先做一个决定才能继续");
    expect(banner).not.toHaveTextContent("needs_business_decision");
  });

  // CF-HDE-R11（M53）：头部提示与 status（R9）同源。真机里会话已在模型上连续
  // 调用，旧实现因为一条陈旧的 `waiting_system` 投影一直显示"等待自动重试"。
  it("a live stream never says it is waiting to retry (M53)", () => {
    render(
      <MessageList
        messages={turn({ objectiveStatus: "waiting_system", nextObservationAt: Date.now() + 30_000, label: "正在调用模型" })}
        streaming
        cwd={null}
      />,
    );
    const banner = screen.getByTestId("turn-activity-progress");
    expect(banner).not.toHaveTextContent("等待");
    expect(banner).not.toHaveTextContent("重试");
    expect(banner).toHaveTextContent("正在调用模型");
  });

  it("a session the server says is running shows progress, not a settled projection", () => {
    render(
      <MessageList
        messages={turn({ objectiveStatus: "waiting_system", nextObservationAt: Date.now() + 30_000, label: "正在调用模型" })}
        streaming={false}
        sessionIsRunning
        cwd={null}
      />,
    );
    const banner = screen.getByTestId("turn-activity-progress");
    expect(banner).toHaveTextContent("正在调用模型");
    expect(banner).not.toHaveTextContent("等待");
  });

  it("follows one synthetic state transition: running -> waiting -> completed", () => {
    const running = turn({ objectiveStatus: "active", label: "正在执行命令" });
    const { rerender } = render(<MessageList messages={running} streaming cwd={null} />);
    expect(screen.getByTestId("turn-activity-progress")).toHaveTextContent("正在执行命令");
    expect(screen.getByTestId("turn-activity-progress")).not.toHaveTextContent("等待");

    rerender(
      <MessageList
        messages={turn({ objectiveStatus: "waiting_system", nextObservationAt: Date.now() + 30_000 })}
        streaming={false}
        cwd={null}
      />,
    );
    expect(screen.getByTestId("turn-activity-progress")).toHaveTextContent("约 30 秒后重试");

    rerender(
      <MessageList
        messages={turn({ objectiveStatus: "completed", label: "系统仍在处理", terminalReason: "objective_completed" })}
        streaming={false}
        cwd={null}
      />,
    );
    expect(screen.queryByTestId("turn-activity-progress")).not.toBeInTheDocument();
  });
});
