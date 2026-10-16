// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, it, vi } from "vitest";
import { reduceChatStreamEvent, type ChatEventState } from "./chatEvents";
import type { StreamEvent } from "../lib/tauri";

function active(): ChatEventState {
  return { messages: [{ id: "a", rootTurnId: "root", role: "assistant", content: "最终摘要", createdAt: 1000,
    toolCalls: [{ id: "tool", name: "bash", args: "synthetic-test", status: "running" }] }],
    streaming: true, inputTokenTotal: 0, outputTokenTotal: 0, pendingPermission: null, contextUsage: null, compressionToast: null };
}
const late: StreamEvent[] = [
  { type: "tool_result", tool_call_id: "tool", content: "LATE_RESULT", is_error: false, status: "done" },
  { type: "text_delta", content: "LATE_TEXT" },
  { type: "tool_call_start", id: "late-tool", name: "bash", args: {} },
  { type: "error", message: "LATE_ERROR" },
];
afterEach(() => vi.useRealTimers());
describe("CF-M30 terminal output watermark", () => {
  it.each(["completed", "cancelled", "failed_setup"] as const)("ignores output arriving 30 seconds after %s", (status) => {
    vi.useFakeTimers(); vi.setSystemTime(10000);
    const settled = reduceChatStreamEvent(active(), { type: "turn_settled", root_turn_id: "root", run_instance_id: "run", status }, "a");
    vi.advanceTimersByTime(30000);
    for (const event of late) expect(reduceChatStreamEvent(settled, event, "a")).toEqual(settled);
  });
  it("uses a hydrated watermark on any segment of the same root", () => {
    const state = active();
    state.messages.push({ id: "final", rootTurnId: "root", role: "assistant", content: "已结束", createdAt: 2000, turnSettledAt: 3000 });
    expect(reduceChatStreamEvent(state, late[0], "a")).toEqual(state);
  });
  it("rejects old-root output after a new turn starts but accepts new output", () => {
    const state = active(); state.messages[0].turnSettledAt = 3000;
    state.messages.push({ id: "new", rootTurnId: "next", role: "assistant", content: "", createdAt: 4000 });
    const old = { type: "text_delta", content: "LATE_TEXT", source_root_turn_id: "root" } as StreamEvent;
    expect(reduceChatStreamEvent(state, old, "new")).toEqual(state);
    expect(reduceChatStreamEvent(state, { type: "text_delta", content: "正常输出" }, "new").messages[1].content).toBe("正常输出");
  });
  it("keeps waiting_system output live", () => {
    const waiting = reduceChatStreamEvent(active(), { type: "turn_settled", root_turn_id: "root", run_instance_id: "run", status: "waiting_system" }, "a");
    expect(reduceChatStreamEvent(waiting, late[1], "a").messages[0].content).toContain("LATE_TEXT");
  });
});
