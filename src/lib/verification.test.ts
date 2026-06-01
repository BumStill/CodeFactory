// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect } from "vitest";
import { parseVerification, verificationSummary } from "./verification";

const result = (check: string, passed: boolean) => ({
  check,
  passed,
  output: passed ? "" : "boom",
  duration_ms: 12,
});

describe("parseVerification", () => {
  it("returns null for missing / empty input", () => {
    expect(parseVerification(null)).toBeNull();
    expect(parseVerification(undefined)).toBeNull();
    expect(parseVerification("")).toBeNull();
  });

  it("returns null for malformed JSON or non-array", () => {
    expect(parseVerification("{not json")).toBeNull();
    expect(parseVerification('{"check":"x"}')).toBeNull(); // object, not array
  });

  it("parses a valid array of results", () => {
    const out = parseVerification(JSON.stringify([result("cargo test", true)]));
    expect(out).toHaveLength(1);
    expect(out![0].check).toBe("cargo test");
    expect(out![0].passed).toBe(true);
  });
});

describe("verificationSummary", () => {
  it("is null when there are no results", () => {
    expect(verificationSummary(null)).toBeNull();
    expect(verificationSummary("[]")).toBeNull();
  });

  it("reports allPassed when every check passes", () => {
    const raw = JSON.stringify([result("a", true), result("b", true)]);
    expect(verificationSummary(raw)).toEqual({ total: 2, passed: 2, allPassed: true });
  });

  it("reports the failed count and allPassed=false when any check fails", () => {
    const raw = JSON.stringify([result("a", true), result("b", false), result("c", false)]);
    expect(verificationSummary(raw)).toEqual({ total: 3, passed: 1, allPassed: false });
  });
});
