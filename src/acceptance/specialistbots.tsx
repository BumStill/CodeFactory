// SPDX-License-Identifier: Apache-2.0
// Production Workspace; synthetic IPC/session only. Never invokes a real backend.
import { createRoot } from "react-dom/client";
import { WorkspacePage } from "../pages/Workspace/WorkspacePage";
import { useChatStore } from "../stores/chat";
import "../styles/globals.css";

useChatStore.getState().beginDraft({ cwd: "/acceptance/project-a" });
function App() {
  const draft = useChatStore((s) => s.draftSession);
  return <main className="h-screen flex flex-col bg-surface-0 text-gray-200">
    <button onClick={() => useChatStore.getState().beginDraft({ cwd: "/acceptance/project-b" })}>验收：切换项目</button>
    <div className="flex-1 min-h-0"><WorkspacePage sessionId={draft?.id ?? ""} onNewConversation={() => {}} onOpenSettings={() => {}} onOpenSession={() => {}} /></div>
  </main>;
}
createRoot(document.getElementById("root")!).render(<App />);
