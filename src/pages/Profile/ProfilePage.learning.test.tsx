// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

import type { LearningEvent } from "../../stores/learning";
import { LearningEventCard } from "./ProfilePage";

describe("LearningEventCard — cross-session evidence review", () => {
  it("shows the real support unit and keeps accept/reject human-gated", () => {
    const onAccept = vi.fn();
    const onReject = vi.fn();
    const event: LearningEvent = {
      id: "pattern-1",
      session_id: "",
      cwd: "/proj",
      observation: "edit_file is flaky across sessions",
      suggestion: "Check edit_file preconditions before retrying.",
      status: "pending",
      created_at: "2026-07-14T00:00:00Z",
      decided_at: null,
      kind: "pattern",
      pref_key: null,
      pref_value: null,
      support_count: 2,
      evidence_json: JSON.stringify({
        detector: "tool_reliability",
        support_unit: "sessions",
        session_count: 2,
        total_calls: 8,
        errors: 2,
        rate: 25,
      }),
    };

    render(
      <LearningEventCard
        event={event}
        busy={false}
        onAccept={onAccept}
        onReject={onReject}
      />,
    );

    expect(screen.getByText("模式 · 2 个 session")).toBeInTheDocument();
    expect(
      screen.getByText("证据：2 个 session · 8 次调用 · 2 次错误 · 25%"),
    ).toBeInTheDocument();
    expect(onAccept).not.toHaveBeenCalled();
    expect(onReject).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: /采纳并写入记忆/ }));
    expect(onAccept).toHaveBeenCalledTimes(1);
    expect(onReject).not.toHaveBeenCalled();
  });

  it.each([
    ["tool_reliability", { total: 8, errors: 2, rate: 25 }],
    ["retry_prone", { task_count: 4 }],
  ])("keeps legacy %s support counts neutral", (detector, legacyEvidence) => {
    const event: LearningEvent = {
      id: `legacy-${detector}`,
      session_id: "",
      cwd: "/proj",
      observation: "Legacy pattern",
      suggestion: "Review the legacy evidence.",
      status: "pending",
      created_at: "2026-07-14T00:00:00Z",
      decided_at: null,
      kind: "pattern",
      pref_key: null,
      pref_value: null,
      support_count: 8,
      evidence_json: JSON.stringify({ detector, ...legacyEvidence }),
    };

    render(
      <LearningEventCard
        event={event}
        busy={false}
        onAccept={vi.fn()}
        onReject={vi.fn()}
      />,
    );

    expect(screen.getByText("模式 · 8 条证据")).toBeInTheDocument();
    expect(screen.getByText(/证据：8 条支持证据/)).toBeInTheDocument();
    expect(screen.queryByText(/8 个 session/)).not.toBeInTheDocument();
  });
});
