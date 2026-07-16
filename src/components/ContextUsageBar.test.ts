// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { formatContextTokens } from "./ContextUsageBar";

describe("formatContextTokens", () => {
  it("keeps the official 1.05M capacity precise instead of rounding it to 1.1M", () => {
    expect(formatContextTokens(1_050_000)).toBe("1.05M");
  });
});
