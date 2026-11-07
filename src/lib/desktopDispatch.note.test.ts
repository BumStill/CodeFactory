// SPDX-License-Identifier: Apache-2.0
//
// M265: the rejection's reason must ride WITH the rejection. An orchestrator that
// denies an approval and then sends a separate steer leaves the model reading a
// reasonless denial first — and it usually retries the same thing. These tests
// pin the dispatch half: `note` is optional, trimmed, length-limited, and never
// changes the decision itself.
import { describe, expect, it, vi } from "vitest";
import { applyDispatchRequest, type DispatchHandlers } from "./desktopDispatch";

function handlers(
  resolveApproval: DispatchHandlers["resolveApproval"],
): DispatchHandlers {
  return {
    createAndSend: vi.fn(),
    send: vi.fn(),
    setPermission: vi.fn(),
    status: vi.fn(),
    setModel: vi.fn(),
    switchSession: vi.fn(),
    stop: vi.fn(),
    listApprovals: vi.fn(async () => []),
    resolveApproval,
  } as unknown as DispatchHandlers;
}

function resolveBody(request: Record<string, unknown>) {
  return applyDispatchRequest(
    { request_id: "r-1", operation: "resolve_approval", request },
    handlers(vi.fn(async () => ({ ok: true }))),
  );
}

describe("M265 resolve_approval note", () => {
  it("hands the trimmed note to the same permission path the UI uses", async () => {
    const resolveApproval = vi.fn(async () => ({ ok: true }));
    const outcome = await applyDispatchRequest(
      {
        request_id: "r-1",
        operation: "resolve_approval",
        request: { approval_id: "intent-1", approve: false, note: "  keep the stash, do not reset  " },
      },
      handlers(resolveApproval),
    );

    expect(outcome).toEqual({ ok: true, result: { ok: true } });
    expect(resolveApproval).toHaveBeenCalledWith({
      approvalId: "intent-1",
      approve: false,
      note: "keep the stash, do not reset",
    });
  });

  it("keeps a note-less denial exactly as it was before M265", async () => {
    const resolveApproval = vi.fn(async () => ({ ok: true }));
    await applyDispatchRequest(
      { request_id: "r-1", operation: "resolve_approval", request: { approval_id: "intent-1", approve: false } },
      handlers(resolveApproval),
    );
    expect(resolveApproval).toHaveBeenCalledWith({ approvalId: "intent-1", approve: false, note: undefined });
  });

  it("treats a blank note as no note instead of sending empty text to the model", async () => {
    const resolveApproval = vi.fn(async () => ({ ok: true }));
    await applyDispatchRequest(
      { request_id: "r-1", operation: "resolve_approval", request: { approval_id: "intent-1", approve: false, note: "   " } },
      handlers(resolveApproval),
    );
    expect(resolveApproval).toHaveBeenCalledWith({ approvalId: "intent-1", approve: false, note: undefined });
  });

  it("refuses an over-long note instead of silently truncating it", async () => {
    const outcome = await resolveBody({ approval_id: "intent-1", approve: false, note: "x".repeat(1001) });
    expect(outcome.ok).toBe(false);
    if (outcome.ok) throw new Error("unreachable");
    expect(outcome.error.code).toBe("invalid_request");
    expect(outcome.error.message).toContain("note");
  });

  it("accepts a note at exactly the limit", async () => {
    const resolveApproval = vi.fn(async () => ({ ok: true }));
    const outcome = await applyDispatchRequest(
      { request_id: "r-1", operation: "resolve_approval", request: { approval_id: "intent-1", approve: false, note: "x".repeat(1000) } },
      handlers(resolveApproval),
    );
    expect(outcome.ok).toBe(true);
  });

  it("still refuses a non-string note", async () => {
    const outcome = await resolveBody({ approval_id: "intent-1", approve: false, note: 42 });
    expect(outcome.ok).toBe(false);
    if (outcome.ok) throw new Error("unreachable");
    expect(outcome.error.code).toBe("invalid_request");
  });

  it("carries a note on an approval without turning it into a grant", async () => {
    const resolveApproval = vi.fn(async () => ({ ok: true }));
    await applyDispatchRequest(
      { request_id: "r-1", operation: "resolve_approval", request: { approval_id: "intent-1", approve: true, note: "read-only, fine" } },
      handlers(resolveApproval),
    );
    expect(resolveApproval).toHaveBeenCalledWith({ approvalId: "intent-1", approve: true, note: "read-only, fine" });
  });
});
