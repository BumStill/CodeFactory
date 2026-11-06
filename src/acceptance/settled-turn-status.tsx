// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for M254: a settled turn never says it is running.
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import "../styles/globals.css";
import { MessageList } from "../components/MessageList";
import type { UIMessage } from "../stores/chatEvents";

const params = new URLSearchParams(location.search);
document.documentElement.dataset.theme = params.get("theme") === "light" ? "light" : "dark";
mockIPC(() => null);
const now = Date.now();
const activity = (kind: string, rootTurnId: string) => ({
  rootTurnId, revision: 1, phase: "working", status: "active", kind, label: "运行中",
  waitingReason: null, updatedAt: now, terminalReason: null,
}) as UIMessage["turnActivity"];
const user = (id: string, at: number): UIMessage => ({ id, role: "user", content: "继续", createdAt: at });
const answer = (id: string, at: number, extra: Partial<UIMessage>): UIMessage => ({
  id, role: "assistant", content: "合成回答：PR 已合并。", createdAt: at, ...extra,
});
const cases: Record<string, UIMessage[]> = {
  settled: [user("root", now - 120_000), answer("a", now - 100_000, { turnSettledAt: now - 4_000, turnActivity: activity("thin_running", "root") })],
  waiting: [user("root", now - 120_000), answer("a", now - 100_000, { turnSettledAt: now - 4_000, turnActivity: activity("thin_waiting_wake", "root") })],
  running: [user("root", now - 120_000), answer("a", now - 100_000, { turnActivity: activity("thin_running", "root") })],
};
createRoot(document.getElementById("root")!).render(
  <div className="flex h-screen flex-col bg-surface-primary">
    <MessageList messages={cases[params.get("case") ?? "settled"]} streaming={false} cwd={null} conversationKey="synthetic" />
  </div>,
);
