// SPDX-License-Identifier: Apache-2.0
// TEMPORARY — reverted in the next commit on this branch.
//
// Proof for PR #299: a red Vitest step must no longer skip the Rust suite.
// Reading the workflow YAML cannot show that; only a run where Vitest actually
// fails can. This forces that failure so the run's step outcomes are evidence.
import { describe, it, expect } from "vitest";

describe("temporary CI proof", () => {
  it("fails on purpose so the Rust steps have something to survive", () => {
    expect("vitest is red").toBe("but cargo must still run");
  });
});
