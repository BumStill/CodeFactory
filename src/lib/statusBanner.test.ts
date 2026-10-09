// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";

import {
  containsInternalVocabulary,
  formatHumanElapsed,
  formatRetryHint,
  statusBannerView,
} from "./statusBanner";

const NOW = 1_000_000;

describe("formatRetryHint (CF-RSB-R2)", () => {
  it.each([
    ["imminent", NOW + 400, "马上重试"],
    ["exactly now", NOW, "马上重试"],
    ["overdue", NOW - 30_000, "马上重试"],
    ["future seconds", NOW + 12_000, "约 12 秒后重试"],
    ["future minutes", NOW + 5 * 60_000, "约 5 分钟后重试"],
    ["future hours", NOW + 3 * 3_600_000, "约 3 小时后重试"],
    ["unknown (null)", null, null],
    ["unknown (undefined)", undefined, null],
    ["non-finite", Number.NaN, null],
  ] as const)("renders %s", (_name, at, expected) => {
    expect(formatRetryHint(at as number | null | undefined, NOW)).toBe(expected);
  });

  it("never renders a raw millisecond duration or a negative time", () => {
    for (const at of [NOW, NOW + 1, NOW + 999, NOW + 1_000, NOW + 30_000, NOW - 90_000, NOW + 90 * 60_000]) {
      const hint = formatRetryHint(at, NOW);
      if (hint) {
        expect(hint).not.toMatch(/\d+\s*ms/i);
        expect(hint).not.toMatch(/-\d/);
      }
    }
  });
});

describe("containsInternalVocabulary (CF-RSB-R1)", () => {
  it.each([
    "objective-supervisor:chat",
    "objective_failed",
    "technical_recovery_exhausted",
    "正在恢复模型连接",
    "等待退避窗口结束",
    "下次观察",
    "generation 3",
    "route fallback",
  ])("flags %s", (text) => {
    expect(containsInternalVocabulary(text)).toBe(true);
  });

  it.each([
    "正在执行",
    "正在执行命令",
    "马上重试",
    "约 12 秒后重试",
    "系统正在等待自动重试",
    "需要你先授权才能继续",
    "验证证据不足",
  ])("accepts %s", (text) => {
    expect(containsInternalVocabulary(text)).toBe(false);
  });
});

describe("formatHumanElapsed (CF-RSB-R2)", () => {
  it("uses human units and never a raw millisecond value", () => {
    expect(formatHumanElapsed(500)).toBe("不到 1 秒");
    expect(formatHumanElapsed(-10)).toBe("不到 1 秒");
    expect(formatHumanElapsed(25_000)).toBe("25.0s");
    for (const ms of [0, 500, 999, 1_000, 25_000, 3_600_000]) {
      expect(formatHumanElapsed(ms)).not.toMatch(/\d+\s*ms/i);
    }
  });
});

describe("statusBannerView (CF-RSB-R3 / R4)", () => {
  it("shows plain running text while the system actually works", () => {
    const view = statusBannerView({
      startedAt: 0,
      nowMs: 25_000,
      activity: { objectiveStatus: "active", label: "正在执行命令", waitingReason: null, nextObservationAt: null },
    });
    expect(view?.text).toBe("正在执行命令");
    expect(view?.waitHint).toBeNull();
    expect(view?.tone).toBe("progress");
  });

  it("shows waiting plus a truthful estimate while waiting to retry", () => {
    const view = statusBannerView({
      startedAt: 0,
      nowMs: 0,
      activity: { objectiveStatus: "waiting_system", label: null, waitingReason: null, nextObservationAt: 12_000 },
    });
    expect(view?.text).toContain("等待");
    expect(view?.waitHint).toBe("约 12 秒后重试");
  });

  it("says 马上重试 for an overdue retry instead of 0ms, and never leaks the owner", () => {
    const view = statusBannerView({
      startedAt: 0,
      nowMs: 25_000,
      activity: {
        objectiveStatus: "waiting_system",
        label: "正在执行命令",
        waitingReason: null,
        recoveryOwner: "objective-supervisor:chat",
        nextObservationAt: 25_000,
      },
    });
    expect(view?.waitHint).toBe("马上重试");
    expect(JSON.stringify(view)).not.toContain("objective-supervisor");
    expect(JSON.stringify(view)).not.toMatch(/0ms/);
  });

  it("hides the estimate when it is unknown", () => {
    const view = statusBannerView({
      startedAt: 0,
      nowMs: 0,
      activity: { objectiveStatus: "waiting_system", label: null, waitingReason: null, nextObservationAt: null },
    });
    expect(view).not.toBeNull();
    expect(view?.waitHint).toBeNull();
  });

  it("drops a raw internal label and falls back to plain words", () => {
    const view = statusBannerView({
      startedAt: 0,
      nowMs: 25_000,
      activity: {
        objectiveStatus: "active",
        label: "route objective-supervisor:chat",
        waitingReason: null,
        recoveryOwner: "objective-supervisor:chat",
        nextObservationAt: null,
      },
    });
    expect(view).not.toBeNull();
    expect(view?.text).toBe("正在执行");
    expect(
      containsInternalVocabulary(`${view?.text} ${view?.detail ?? ""} ${view?.waitHint ?? ""}`),
    ).toBe(false);
  });

  it("tells the user plainly what to do when they must act", () => {
    const view = statusBannerView({
      startedAt: 0,
      nowMs: 0,
      activity: {
        objectiveStatus: "waiting_authorization",
        label: null,
        waitingReason: "authorization_required",
        nextObservationAt: null,
      },
    });
    expect(view?.tone).toBe("warning");
    expect(view?.text).toBe("需要你先授权才能继续");
    expect(view?.waitHint).toBeNull();
  });

  it.each(["completed", "failed", "cancelled"])(
    "shows no banner once the task ended as %s",
    (objectiveStatus) => {
      expect(
        statusBannerView({
          startedAt: 0,
          nowMs: 5_000,
          activity: { objectiveStatus, label: "系统仍在处理", waitingReason: null, nextObservationAt: null },
        }),
      ).toBeNull();
    },
  );

  it("shows no banner for a terminal waiting reason", () => {
    expect(
      statusBannerView({
        startedAt: 0,
        nowMs: 5_000,
        activity: {
          objectiveStatus: "waiting_system",
          label: null,
          waitingReason: "technical_recovery_exhausted",
          terminalReason: "technical_recovery_exhausted",
          nextObservationAt: null,
        },
      }),
    ).toBeNull();
  });

  it("never renders internal vocabulary in any banner state", () => {
    const states = [
      { objectiveStatus: "active", label: "正在执行命令", waitingReason: null, nextObservationAt: null },
      { objectiveStatus: "waiting_system", label: null, waitingReason: null, nextObservationAt: NOW + 90_000 },
      { objectiveStatus: "waiting_system", label: null, waitingReason: null, nextObservationAt: NOW - 1 },
      { objectiveStatus: "waiting_system", label: "已切换到备用模型 route", waitingReason: "等待退避窗口结束", recoveryOwner: "objective-supervisor:chat", nextObservationAt: NOW + 5_000 },
      { objectiveStatus: "active", label: "正在恢复模型连接", waitingReason: "objective_failed_observed", recoveryOwner: "objective-supervisor:provider", nextObservationAt: null },
      { objectiveStatus: "waiting_authorization", label: null, waitingReason: "authorization_required", nextObservationAt: null },
      { objectiveStatus: "waiting_business_decision", label: null, waitingReason: "needs_business_decision", nextObservationAt: null },
    ] as const;
    for (const activity of states) {
      const view = statusBannerView({ startedAt: 0, nowMs: NOW, activity });
      if (!view) continue;
      const rendered = [view.text, view.detail ?? "", view.waitHint ?? "", view.elapsed].join(" ");
      expect(containsInternalVocabulary(rendered)).toBe(false);
    }
  });
});
