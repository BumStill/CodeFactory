// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect } from "vitest";
import { formatDuration } from "./duration";

describe("formatDuration", () => {
  it("renders sub-second values in milliseconds", () => {
    expect(formatDuration(0)).toBe("0ms");
    expect(formatDuration(1)).toBe("1ms");
    expect(formatDuration(850)).toBe("850ms");
    expect(formatDuration(999)).toBe("999ms");
  });

  it("renders seconds with one decimal under a minute", () => {
    expect(formatDuration(1000)).toBe("1.0s");
    expect(formatDuration(12_700)).toBe("12.7s");
    expect(formatDuration(59_900)).toBe("59.9s");
  });

  it("renders m:ss (zero-padded) under an hour", () => {
    expect(formatDuration(60_000)).toBe("1m00s");
    expect(formatDuration(67_000)).toBe("1m07s");
    expect(formatDuration(150_000)).toBe("2m30s");
    expect(formatDuration(3_599_000)).toBe("59m59s");
  });

  it("renders h:mm (zero-padded) at an hour or more", () => {
    expect(formatDuration(3_600_000)).toBe("1h00m");
    expect(formatDuration(3_720_000)).toBe("1h02m");
  });

  it("clamps negatives and non-finite input to 0ms", () => {
    expect(formatDuration(-5)).toBe("0ms");
    expect(formatDuration(NaN)).toBe("0ms");
    expect(formatDuration(Infinity)).toBe("0ms");
  });
});
