// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { contextUsagePresentation, formatContextTokens } from "./ContextUsageBar";

describe("formatContextTokens", () => {
  it("keeps the official 1.05M capacity precise instead of rounding it to 1.1M", () => {
    expect(formatContextTokens(1_050_000)).toBe("1.05M");
  });

  it("keeps ordinary context usage neutral and only escalates real pressure", () => {
    expect(contextUsagePresentation(58).tone).toBe("progress");
    expect(contextUsagePresentation(58).label).toBe("上下文充足");
    expect(contextUsagePresentation(75).tone).toBe("warning");
    expect(contextUsagePresentation(90).tone).toBe("danger");
  });
});
