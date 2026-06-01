// SPDX-License-Identifier: Apache-2.0
import type { VerificationResult } from "./tauri";

/** Parse a TaskRun.verification_results JSON string into the per-check list.
 *  Returns null for missing/empty/malformed/non-array input. */
export function parseVerification(
  raw: string | null | undefined,
): VerificationResult[] | null {
  if (!raw) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? (parsed as VerificationResult[]) : null;
  } catch {
    return null;
  }
}

export interface VerificationSummary {
  total: number;
  passed: number;
  allPassed: boolean;
}

/** Aggregate pass/fail counts for a TaskRun's verification results, or null when
 *  there are none yet (so callers can show nothing rather than "0/0"). */
export function verificationSummary(
  raw: string | null | undefined,
): VerificationSummary | null {
  const list = parseVerification(raw);
  if (!list || list.length === 0) return null;
  const passed = list.filter((r) => r.passed).length;
  return { total: list.length, passed, allPassed: passed === list.length };
}
