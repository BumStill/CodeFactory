// SPDX-License-Identifier: Apache-2.0
import React from "react";
import { createRoot } from "react-dom/client";
import "../styles/globals.css";
import { WorkspaceDeliveryStatus } from "../components/WorkspaceDeliveryStatus";
import { PermissionDialog } from "../components/PermissionDialog";
import { SessionSidebar } from "../components/SessionSidebar";
import { useChatStore, freshRuntime, activeRuntime } from "../stores/chat";
import type { Session } from "../lib/tauri";

const sessions: Session[] = ["A", "B"].map(id => ({ id, title: `合成会话 ${id}`, cwd: "/synthetic/project", model_id: "synthetic", permission_mode: "trusted", created_at: 1, updated_at: 1, total_input_tokens: 0, total_output_tokens: 0 }));
const pending = { intentId: "synthetic-approval", toolCallId: "synthetic-call", toolName: "bash", args: { command: "printf synthetic" }, expiresAt: Date.now() + 600000 };
(window as any).__TAURI_INTERNALS__ = { invoke: async (cmd: string, args: any) => {
  if (cmd === "get_session") return sessions.find(s => s.id === args.sessionId);
  if (cmd === "get_message_page") return { messages: [], has_more: false };
  return false;
}};
useChatStore.setState({ sessions, activeSession: sessions[0], runtime: { A: { ...freshRuntime(), pendingPermission: pending }, B: freshRuntime() }, loadSessions: async () => sessions });
function App() {
  const current = useChatStore(s => s.activeSession);
  const request = useChatStore(s => activeRuntime(s).pendingPermission);
  const [merged, setMerged] = React.useState(false);
  return <main className="min-h-screen bg-bg p-6 text-gray-200">
    <h1>遗留收口：合成数据验收</h1>
    <section className="my-4"><WorkspaceDeliveryStatus cwd="/synthetic/project" currentBranch="synthetic" messages={[]} deliveryState={{ unavailable: false, snapshot: { remote_available: true, pr: { number: 175, title: "Synthetic", state: merged ? "merged" : "open", draft: false, head_branch: "synthetic", base_branch: "main", head_sha: "abc", merge_commit_sha: merged ? "def" : null, url: "https://example.invalid/pull/175" }, ci_status: "failure", release: null, error: null } }}/><button onClick={() => setMerged(true)}>模拟合并刷新</button></section>
    <div className="w-80 h-96"><SessionSidebar currentSessionId={current?.id ?? ""} onOpenSession={id => { void useChatStore.getState().selectSession(id); }} onNewConversation={() => {}}/></div>
    <button onClick={() => { void useChatStore.getState().selectSession("B"); }}>切到 B</button>
    <button onClick={() => { void useChatStore.getState().selectSession("A"); }}>切回 A</button>
    <p data-testid="active-session">{current?.id}</p>
    {request && <PermissionDialog request={request} trusted onAllow={() => {}} onDeny={() => {}} onAllowFullAccess={() => {}}/>}
  </main>;
}
createRoot(document.getElementById("root")!).render(<App/>);
