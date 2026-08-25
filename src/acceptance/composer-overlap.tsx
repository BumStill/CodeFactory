// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the new-session composer staying reachable.
//
// Reproduces the WorkspacePage centre column verbatim: MessageList's empty
// state (WelcomeScreen) above a `shrink-0` composer. In a short window the
// welcome content is taller than the column, so it can only stay clear of the
// composer if MessageList contains it. jsdom cannot catch this — it computes
// no layout — which is why this gate runs in a real browser.

import React from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageList } from "../components/MessageList";

function AcceptanceApp() {
  return (
    <div className="flex h-screen flex-col bg-surface-0">
      <div className="relative flex min-h-0 flex-1">
        <main
          aria-label="Composer overlap acceptance"
          className="flex min-w-0 flex-1 flex-col bg-surface-2"
        >
          <MessageList
            messages={[]}
            streaming={false}
            turnActive={false}
            cwd={null}
            conversationKey="composer-overlap-acceptance"
          />
          <div data-testid="workspace-composer-shell" className="shrink-0 bg-surface-2 px-3 pb-3 pt-2">
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

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AcceptanceApp />
  </React.StrictMode>,
);
