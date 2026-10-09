// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the sidebar session-title states (M40).
//
// Renders the real `SessionSidebar` with one synthetic row per title state the
// spec calls out — generated, admitted fallback, hand-renamed manual, an
// over-long Chinese title and an over-long English title — so a headless
// browser can assert truncation (no horizontal overflow) in both themes.

import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { SessionSidebar } from "../components/SessionSidebar";
import { useChatStore } from "../stores/chat";
import type { Session } from "../lib/tauri";

const mk = (over: Partial<Session>): Session => ({
  id: "x",
  title: "",
  cwd: "/synthetic/acme-app",
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

// Synthetic first-message titles, one per lifecycle state. No real session id,
// path or user text is used.
export const LONG_ZH_TITLE =
  "重构支付网关的超时重试与幂等键生成逻辑并补齐并发回归测试与可观测性埋点";
export const LONG_EN_TITLE =
  "Investigate intermittent sidebar title regression across parallel session creation and provider timeouts";

const sessions: Session[] = [
  mk({ id: "gen", title: "会话命名优化", updated_at: 600 }),
  mk({ id: "fb", title: "登录问题排查", updated_at: 500 }),
  mk({ id: "man", title: "手工名称", updated_at: 400 }),
  mk({ id: "long-zh", title: LONG_ZH_TITLE, updated_at: 300 }),
  mk({ id: "long-en", title: LONG_EN_TITLE, updated_at: 200 }),
  mk({ id: "ph", title: "新会话", updated_at: 100 }),
];

useChatStore.setState({
  sessions,
  activeSession: sessions[0],
  draftSession: null,
  runtime: {},
  loadSessions: async () => sessions,
  deleteSession: async () => {},
  renameSession: async () => {},
});

function SessionTitleAcceptance() {
  return (
    <main
      className="h-screen w-[280px] bg-surface-0 text-gray-200"
      aria-label="Session title acceptance"
    >
      <SessionSidebar
        currentSessionId="gen"
        onOpenSession={() => {}}
        onNewConversation={() => {}}
      />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<SessionTitleAcceptance />);
