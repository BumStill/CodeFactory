// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for M44: what the user types reaches the agent
// unchanged (CF-INP-R1 / CF-INP-R2).
//
// The defect it guards was invisible to jsdom: macOS WebKit applies its "smart"
// text substitution (smart dashes `--` → `—`, smart quotes, auto-correction)
// inside an editable element unless that element opts out with the
// `autocorrect` / `autocapitalize` / `spellcheck` attributes. jsdom has no such
// engine, so a unit test can only assert the attributes exist; only a real
// engine can be asked what the element actually does with a keystroke.
//
// This entry mounts the production `MessageInput` — not a copy — and publishes
// every `onSend` payload on `window.__composerSent` plus `#sent-log`, so the
// headless script can compare the sent content byte-for-byte with the input.

import React, { useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageInput } from "../components/MessageInput";

declare global {
  interface Window {
    __composerSent: string[];
  }
}

function AcceptanceApp() {
  const [sent, setSent] = useState<string[]>([]);
  window.__composerSent = sent;
  return (
    <div className="flex h-screen flex-col bg-surface-0">
      <main
        aria-label="Composer verbatim acceptance"
        className="flex min-h-0 flex-1 flex-col justify-end bg-surface-2 px-3 pb-3"
      >
        <pre
          id="sent-log"
          data-testid="sent-log"
          className="mb-2 max-h-40 overflow-auto rounded border border-border p-2 text-note text-gray-400"
        >
          {JSON.stringify(sent)}
        </pre>
        <MessageInput
          onSend={(text) => setSent((previous) => [...previous, text])}
          onCancel={() => {}}
          streaming={false}
          disabled={false}
          cwd="/tmp/composer-verbatim-acceptance"
        />
      </main>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AcceptanceApp />
  </React.StrictMode>,
);
