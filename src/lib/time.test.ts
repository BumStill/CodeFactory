// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect } from "vitest";
import { formatRelativeTime } from "./time";

// A fixed "now" in milliseconds so the assertions are deterministic.
const NOW = 1_780_000_000_000;
const SEC = 1000;
const MIN = 60 * SEC;
const HR = 60 * MIN;
const DAY = 24 * HR;

describe("formatRelativeTime", () => {
  it("shows 刚刚 within the last minute (and at now)", () => {
    expect(formatRelativeTime(NOW, NOW)).toBe("刚刚");
    expect(formatRelativeTime(NOW - 30 * SEC, NOW)).toBe("刚刚");
  });

  it("shows whole minutes under an hour", () => {
    expect(formatRelativeTime(NOW - MIN, NOW)).toBe("1 分钟前");
    expect(formatRelativeTime(NOW - 59 * MIN, NOW)).toBe("59 分钟前");
  });

  it("shows whole hours under a day", () => {
    expect(formatRelativeTime(NOW - HR, NOW)).toBe("1 小时前");
    expect(formatRelativeTime(NOW - 2 * HR, NOW)).toBe("2 小时前");
  });

  it("shows whole days under a week", () => {
    expect(formatRelativeTime(NOW - DAY, NOW)).toBe("1 天前");
    expect(formatRelativeTime(NOW - 6 * DAY, NOW)).toBe("6 天前");
  });

  it("falls back to a locale date at a week or older", () => {
    const old = NOW - 30 * DAY;
    expect(formatRelativeTime(old, NOW)).toBe(new Date(old).toLocaleDateString());
  });

  it("regression: treats the input as milliseconds, not seconds", () => {
    // 2 hours ago expressed in MILLISECONDS must read as hours. The old
    // `ts * 1000` bug made any real (millisecond) timestamp register as 刚刚.
    expect(formatRelativeTime(NOW - 2 * HR, NOW)).toBe("2 小时前");
    expect(formatRelativeTime(NOW - 3 * DAY, NOW)).not.toBe("刚刚");
  });

  it("never throws on a future timestamp (clock skew) — clamps to 刚刚", () => {
    expect(formatRelativeTime(NOW + 5 * MIN, NOW)).toBe("刚刚");
  });
});
