// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for preserving expanded project groups.

import React from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { ProjectPmEntry } from "../pages/Workspace/ProjectPmEntry";
import { SessionSidebar } from "../components/SessionSidebar";
import { useChatStore } from "../stores/chat";
import type { Session } from "../lib/tauri";

const mk = (over: Partial<Session>): Session => ({
  id: "x",
  title: "",
  cwd: "/x",
  endpoint_id: "openrouter",
  model_id: "test-model",
  model_policy: "prefer",
  permission_mode: "standard",
  created_at: 1,
  updated_at: 1,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
  ...over,
});

const sessions: Session[] = [
  mk({ id: "p1a", title: "CodeFactory 主线", cwd: "/code/CodeFactory", updated_at: 400 }),
  mk({ id: "q1", title: "改图脚本", cwd: "/home/.codefactory/quick/q1", updated_at: 300, kind: "quick" }),
  mk({ id: "p1b", title: "CodeFactory 旧会话", cwd: "/code/CodeFactory", updated_at: 200 }),
  mk({ id: "p2", title: "记账 app", cwd: "/code/ledger", updated_at: 100 }),
];

useChatStore.setState({
  sessions,
  activeSession: sessions[1],
  draftSession: null,
  runtime: {},
  loadSessions: async () => sessions,
  deleteSession: async () => {},
  renameSession: async () => {},
});

function SidebarExpansionAcceptance() {
  const [current, setCurrent] = React.useState("q1");
  const [newConversations, setNewConversations] = React.useState(0);
  const select = (id: string) => { useChatStore.setState({ draftSession: null }); setCurrent(id); };
  return (
    <main className="flex h-screen w-[360px] max-w-full flex-col bg-surface-0 text-gray-200" aria-label="Sidebar expansion acceptance">
      <ProjectPmEntry sessionId={current} />
      <SessionSidebar
        currentSessionId={current}
        onOpenSession={select}
        onNewConversation={() => setNewConversations((n) => n + 1)}
      />
      <nav className="flex shrink-0 flex-wrap gap-2 p-2" aria-label="Synthetic boundary controls">
        <button onClick={() => { const draft = useChatStore.getState().beginDraft(); setCurrent(draft.id); }}>无项目草稿</button>
        <button onClick={() => { const draft = useChatStore.getState().beginDraft({cwd:"/code/CodeFactory",anonymous:true}); setCurrent(draft.id); }}>匿名草稿</button>
        <button onClick={() => select("unbound-project")}>未绑定项目</button>
      </nav>
      <div aria-label="Sidebar expansion probe" data-current-session={current} data-new-conversations={newConversations} />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<SidebarExpansionAcceptance />);
