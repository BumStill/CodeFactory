// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

describe("CF-HDE-R12 durable dispatch integration", () => {
  it("lists persisted approvals rather than the one-per-session UI cache", () => {
    const source = readFileSync("src/pages/Workspace/WorkspacePage.tsx", "utf8");
    const handler = source.slice(source.indexOf("listApprovals: async"), source.indexOf("resolveApproval: async"));
    expect(handler).toContain('invoke<DispatchApproval[]>("list_pending_approvals")');
    expect(handler).not.toContain("store.runtime");
  });
  it("resolves exactly one intent using the existing permission command", () => {
    const source = readFileSync("src/pages/Workspace/WorkspacePage.tsx", "utf8");
    const handler = source.slice(source.indexOf("resolveApproval: async"), source.indexOf("status: async"));
    expect(handler).toContain('invoke("respond_to_permission", { intentId: approvalId, allow: approve })');
  });
});
