// SPDX-License-Identifier: Apache-2.0
//
// Per-session concurrency contract: two sessions can stream at the same time,
// each into its OWN runtime bucket. Switching the active session must not
// interrupt, lose, or cross-wire a background session's in-flight stream.
// This is the whole point of the per-session refactor — the old global
// `streaming` flag made all of this impossible.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore, activeRuntime, freshRuntime } from "./chat";
import { useSettingsStore } from "./settings";
import type { StreamEvent } from "../lib/tauri";

// Capture each session's stream callback so we can simulate backend events
// arriving for a specific session — including a BACKGROUND one.
const streamHandlers: Record<string, (e: StreamEvent) => void> = {};
const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({
  invoke: invokeMock,
  onStream: vi.fn(async (sid: string, cb: (e: StreamEvent) => void) => {
    streamHandlers[sid] = cb;
    return () => {
      delete streamHandlers[sid];
    };
  }),
  onSessionUpdated: vi.fn(async () => () => {}),
  sendMessageAnonymous: vi.fn(async () => {}),
}));

function mkSession(id: string) {
  return {
    id,
    title: id,
    cwd: `/p/${id}`,
    model_id: "m",
    created_at: 0,
    updated_at: 0,
    total_input_tokens: 0,
    total_output_tokens: 0,
    kind: "project",
  };
}

const A = mkSession("A");
const B = mkSession("B");

function rt(id: string) {
  return useChatStore.getState().runtime[id]!;
}
function lastContent(id: string) {
  const msgs = rt(id).messages;
  return msgs[msgs.length - 1]?.content;
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  for (const k of Object.keys(streamHandlers)) delete streamHandlers[k];
  useChatStore.setState({
    sessions: [A, B] as never,
    quickSessions: [],
    activeSession: A as never,
    runtime: { A: freshRuntime(), B: freshRuntime() },
    _unlisten: {},
    _unlistenSessionUpdated: {},
    _streamingMsgId: {},
  });
  useSettingsStore.setState({ settings: null });
});

describe("per-session streaming concurrency", () => {
  it("two sessions stream independently; events route to the right bucket", async () => {
    // Start A streaming (A is active).
    await useChatStore.getState().sendMessage("hi A");
    expect(rt("A").streaming).toBe(true);

    // Foreground B (no DB round-trip in the test), then start B streaming too.
    useChatStore.setState({ activeSession: B as never });
    await useChatStore.getState().sendMessage("hi B");
    expect(rt("B").streaming).toBe(true);

    // Both sessions own a live stream handler.
    expect(typeof streamHandlers.A).toBe("function");
    expect(typeof streamHandlers.B).toBe("function");

    // A token for the BACKGROUND session A must update A only.
    streamHandlers.A({ type: "text_delta", content: "alpha" });
    expect(lastContent("A")).toBe("alpha");
    expect(lastContent("B")).toBe(""); // B's assistant turn untouched

    // A token for B updates B only.
    streamHandlers.B({ type: "text_delta", content: "beta" });
    expect(lastContent("B")).toBe("beta");
    expect(lastContent("A")).toBe("alpha");
  });

  it("finishing a background session leaves the active one streaming", async () => {
    await useChatStore.getState().sendMessage("hi A");
    useChatStore.setState({ activeSession: B as never });
    await useChatStore.getState().sendMessage("hi B");

    // A finishes while B is the foreground session.
    streamHandlers.A({ type: "done", input_tokens: 1, output_tokens: 2 });

    expect(rt("A").streaming).toBe(false);
    expect(rt("B").streaming).toBe(true);
    // The active view (B) is unaffected — still streaming.
    expect(activeRuntime(useChatStore.getState()).streaming).toBe(true);
  });

  it("activeRuntime reflects whichever session is in the foreground", async () => {
    await useChatStore.getState().sendMessage("hi A");
    expect(activeRuntime(useChatStore.getState()).streaming).toBe(true); // A: active + streaming
    useChatStore.setState({ activeSession: B as never });
    expect(activeRuntime(useChatStore.getState()).streaming).toBe(false); // B: idle
  });

  it("keeps remote post-mortem off until the user explicitly opts in", async () => {
    useChatStore.setState({
      activeSession: A as never,
      runtime: {
        A: {
          ...freshRuntime(),
          messages: [{ id: "old", role: "user", content: "earlier", createdAt: 0 }],
        },
        B: freshRuntime(),
      },
    });

    await useChatStore.getState().sendMessage("finish this", "A");
    streamHandlers.A({ type: "done", input_tokens: 1, output_tokens: 2 });

    expect(invokeMock).not.toHaveBeenCalledWith("run_postmortem", expect.anything());
  });

  it("runs the bounded remote post-mortem only after explicit opt-in", async () => {
    useSettingsStore.setState({
      settings: { remote_postmortem_enabled: true } as never,
    });
    useChatStore.setState({
      activeSession: B as never,
      runtime: {
        A: freshRuntime(),
        B: {
          ...freshRuntime(),
          messages: [{ id: "old", role: "user", content: "earlier", createdAt: 0 }],
        },
      },
    });

    await useChatStore.getState().sendMessage("finish this", "B");
    streamHandlers.B({ type: "done", input_tokens: 1, output_tokens: 2 });

    expect(invokeMock).toHaveBeenCalledWith("run_postmortem", {
      sessionId: "B",
      cwd: "/p/B",
    });
  });
});
