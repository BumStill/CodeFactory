// SPDX-License-Identifier: Apache-2.0
//
// learning store contract tests. Covers the user-flow semantics that
// Workspace + Profile depend on:
//   - load fetches and caches per-cwd
//   - accept / reject patch state optimistically + call the right command
//   - subscribe dedups duplicate calls per cwd (so re-renders don't leak)

import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
const listenMock = vi.hoisted(() => vi.fn(async () => () => {}));

vi.mock("../lib/tauri", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

import { useLearningStore, type LearningEvent } from "./learning";

function reset() {
  useLearningStore.setState({ events: {}, loading: {}, _unlisten: {} });
  invokeMock.mockReset();
  listenMock.mockReset();
  listenMock.mockImplementation(async () => () => {});
}

const mkEvent = (id: string, status: LearningEvent["status"] = "pending"): LearningEvent => ({
  id,
  session_id: "s1",
  cwd: "/proj",
  observation: `obs ${id}`,
  suggestion: `sug ${id}`,
  status,
  created_at: "2026-01-01",
  decided_at: status === "pending" ? null : "2026-01-02",
  kind: "memory",
  pref_key: null,
  pref_value: null,
  support_count: 0,
  evidence_json: "{}",
});

describe("learning store", () => {
  beforeEach(reset);

  it("load fetches via list_learning_events and caches per cwd", async () => {
    invokeMock.mockResolvedValueOnce([mkEvent("a"), mkEvent("b")]);
    await useLearningStore.getState().load("/proj");
    expect(invokeMock).toHaveBeenCalledWith("list_learning_events", { cwd: "/proj" });
    expect(useLearningStore.getState().events["/proj"]).toHaveLength(2);
    expect(useLearningStore.getState().loading["/proj"]).toBe(false);
  });

  it("approval calls the versioned Eval command and removes the legacy pending row", async () => {
    invokeMock.mockResolvedValueOnce([mkEvent("a")]); // load
    await useLearningStore.getState().load("/proj");
    invokeMock.mockResolvedValueOnce(undefined); // accept

    await useLearningStore.getState().accept("a", "/proj", false);

    expect(invokeMock).toHaveBeenCalledWith("approve_learning_event", { eventId: "a", autoActivate: false });
    const a = useLearningStore.getState().events["/proj"].find((e) => e.id === "a");
    expect(a).toBeUndefined();
  });

  it("reject calls reject_learning_event and patches local state optimistically", async () => {
    invokeMock.mockResolvedValueOnce([mkEvent("a")]);
    await useLearningStore.getState().load("/proj");
    invokeMock.mockResolvedValueOnce(undefined);

    await useLearningStore.getState().reject("a", "/proj");

    expect(invokeMock).toHaveBeenCalledWith("reject_learning_event", { eventId: "a" });
    expect(useLearningStore.getState().events["/proj"][0].status).toBe("rejected");
  });

  it("subscribe registers ONE listener per cwd even when called multiple times", async () => {
    const off1 = await useLearningStore.getState().subscribe("/proj");
    const off2 = await useLearningStore.getState().subscribe("/proj");
    // listen() called exactly once even with 2 subscribe() calls.
    expect(listenMock).toHaveBeenCalledTimes(1);
    // Both unlisten handles are callable (no throw).
    off1();
    off2();
  });

  it("subscribe sets up the right per-cwd event name", async () => {
    await useLearningStore.getState().subscribe("/a");
    expect(listenMock).toHaveBeenCalledWith("learning_events_updated:/a", expect.any(Function));
    await useLearningStore.getState().subscribe("/b");
    expect(listenMock).toHaveBeenCalledWith("learning_events_updated:/b", expect.any(Function));
  });

  it("backend-fired event triggers a re-load", async () => {
    // Capture the handler registered by listen(). Cast: listenMock was
    // created with a 0-arg default, so overriding with the real 2-arg
    // listener signature needs `as any` to keep strict TS happy without
    // leaking jest-mock typing details everywhere.
    let handler: ((payload: { payload: unknown }) => void) | undefined;
    (listenMock as unknown as {
      mockImplementationOnce: (fn: (...args: unknown[]) => unknown) => unknown;
    }).mockImplementationOnce(
      async (_name: unknown, cb: unknown) => {
        handler = cb as (payload: { payload: unknown }) => void;
        return () => {};
      },
    );
    await useLearningStore.getState().subscribe("/proj");

    // First load fired by subscribe... no it doesn't, subscribe doesn't auto-load.
    // Now simulate a backend event arriving.
    invokeMock.mockResolvedValueOnce([mkEvent("fresh", "pending")]);
    handler?.({ payload: null });
    // Wait one microtask for the load promise.
    await Promise.resolve();
    await Promise.resolve();

    expect(invokeMock).toHaveBeenCalledWith("list_learning_events", { cwd: "/proj" });
  });

  it("mine creates pending patterns through the backend and reloads the review list", async () => {
    const pattern = {
      ...mkEvent("pattern-1"),
      session_id: "",
      kind: "pattern" as const,
      support_count: 2,
      evidence_json: JSON.stringify({
        detector: "tool_reliability",
        support_unit: "sessions",
        session_count: 2,
        total_calls: 8,
        errors: 2,
        rate: 25,
      }),
    };
    invokeMock.mockResolvedValueOnce([pattern]); // mine_cross_session_patterns
    invokeMock.mockResolvedValueOnce([pattern]); // list_learning_events refresh

    const count = await useLearningStore.getState().mine("/proj");

    expect(count).toBe(1);
    expect(invokeMock).toHaveBeenNthCalledWith(1, "mine_cross_session_patterns", {
      cwd: "/proj",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "list_learning_events", {
      cwd: "/proj",
    });
    expect(useLearningStore.getState().events["/proj"]).toEqual([pattern]);
  });
});
