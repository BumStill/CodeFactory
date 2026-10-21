// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { presentChatInvocationError } from "./chatEvents";

describe("M144 local storage contention presentation", () => {
  it.each([
    "Database error: error returned from database: (code: 5) database is locked",
    "provider output checkpoint failed: SQLITE_BUSY_SNAPSHOT",
    "database table is locked",
  ])("never leaks a SQLite diagnostic: %s", (error) => {
    const result = presentChatInvocationError(error);
    expect(result.content).not.toMatch(/database|SQLITE|code: 5|Error:/i);
    expect(result.content).toContain("本地数据");
  });
  it("keeps unrelated errors unchanged", () => {
    expect(presentChatInvocationError("synthetic failure").content).toBe("Error: synthetic failure");
  });
});
