// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for a new session's first impression.
//
// Reproduces the WorkspacePage centre column (MessageList above a `shrink-0`
// composer) and drives the three entry paths from the defect report through the
// REAL chat store: a conversation that ended, a conversation with a turn still
// running, and a conversation whose history has not been hydrated yet. Each path
// first shows a long, scrolled conversation, so the gate can prove that the
// start page does not inherit its scroll offset. jsdom computes no layout and
// cannot see any of this, which is why the assertions live in
// scripts/verify-new-session-start-headless.mjs.

import React from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageList } from "../components/MessageList";
import { activeRuntime, openSessionId, useChatStore } from "../stores/chat";
import type { UIMessage } from "../stores/chatEvents";

const PREVIOUS_SESSION = {
  id: "acceptance-previous-session",
  title: "上一个会话",
  cwd: "/tmp/codefactory-acceptance",
  model_id: "acceptance-model",
  created_at: 0,
  updated_at: 0,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project" as const,
};

/** Long enough that the conversation genuinely overflows a short viewport, so
 *  the previous view really is scrolled when the user starts a new session. */
function longConversation(): UIMessage[] {
  const messages: UIMessage[] = [];
  for (let index = 0; index < 24; index += 1) {
    messages.push({
      id: `u${index}`,
      role: "user",
      content: `第 ${index + 1} 轮：继续完善这个功能，并把验证结果整理出来。`,
      createdAt: index * 2,
    });
    messages.push({
      id: `a${index}`,
      role: "assistant",
      content: `第 ${index + 1} 轮已完成：修改了会话起始页，并补上了对应的验证与截图。`,
      createdAt: index * 2 + 1,
    });
  }
  return messages;
}

type EntryPath = "ended" | "running" | "loading";

function openPreviousConversation(streaming: boolean) {
  useChatStore.setState({
    sessions: [PREVIOUS_SESSION] as never,
    activeSession: PREVIOUS_SESSION as never,
    draftSession: null,
    runtime: {
      [PREVIOUS_SESSION.id]: {
        messages: longConversation(),
        streaming,
        queue: [],
        pendingPermission: null,
      } as never,
    },
  });
}

function startNewSession(path: EntryPath) {
  if (path === "loading") {
    // Selected, but its history has not arrived: the runtime bucket is absent.
    useChatStore.setState({
      sessions: [PREVIOUS_SESSION] as never,
      activeSession: PREVIOUS_SESSION as never,
      draftSession: null,
      runtime: {},
    });
  }
  useChatStore.getState().beginDraft();
}

function AcceptanceApp() {
  const conversationKey = useChatStore(openSessionId);
  const { messages } = useChatStore(activeRuntime);

  return (
    <div className="flex h-screen flex-col bg-surface-0">
      <div className="relative flex min-h-0 flex-1">
        <main
          aria-label="New session start acceptance"
          className="flex min-w-0 flex-1 flex-col bg-surface-2"
        >
          <MessageList
            messages={messages}
            streaming={false}
            turnActive={false}
            cwd={null}
            conversationKey={conversationKey}
          />
          <div
            data-testid="workspace-composer-shell"
            className="shrink-0 bg-surface-2 px-3 pb-3 pt-2"
          >
            <div className="mx-auto w-full max-w-[var(--reading-column)]">
              <div className="rounded-lg border border-border bg-surface-1">
                <div className="flex items-end gap-2 px-3 py-2.5">
                  <textarea
                    aria-label="消息输入"
                    rows={1}
                    className="min-h-8 max-h-[200px] flex-1 resize-none bg-transparent py-1 text-reading leading-6 text-gray-200 outline-none"
                  />
                </div>
              </div>
            </div>
          </div>
        </main>
      </div>
    </div>
  );
}

declare global {
  interface Window {
    __startPageAcceptance?: {
      openPreviousConversation: (streaming: boolean) => void;
      startNewSession: (path: EntryPath) => void;
    };
  }
}

window.__startPageAcceptance = {
  openPreviousConversation,
  startNewSession,
};

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AcceptanceApp />
  </React.StrictMode>,
);
