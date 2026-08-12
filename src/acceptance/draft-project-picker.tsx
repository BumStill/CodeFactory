// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the draft project picker overlay.

import { useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { DraftScopeBar } from "../components/DraftScopeBar";
import { MessageInput } from "../components/MessageInput";
import type { ProjectGroup } from "../lib/projects";

const projects: ProjectGroup[] = [
  { cwd: "/Users/leo/Projects/CodeFactory", name: "CodeFactory", sessions: [], updatedAt: 2 },
  { cwd: "/Users/leo/Projects/AI foundation", name: "AI foundation", sessions: [], updatedAt: 1 },
];

function DraftProjectPickerAcceptance() {
  const [cwd, setCwd] = useState<string | null>(null);
  return (
    <main aria-label="Draft project picker acceptance" className="flex h-screen flex-col bg-surface-0 p-2 text-gray-200 sm:p-8">
      <div className="flex min-h-0 flex-1 items-end justify-center">
        <div
          data-testid="clipped-composer"
          className="w-full max-w-[880px] overflow-hidden"
        >
          <MessageInput
            onSend={() => {}}
            onCancel={() => {}}
            streaming={false}
            disabled={false}
            cwd={cwd}
            toolbar={(
              <DraftScopeBar
                cwd={cwd}
                anonymous={false}
                projects={projects}
                modelPicker={(
                  <button
                    type="button"
                    aria-label="选择下一回合模型：gpt-5.6-sol"
                    className="min-h-[44px] max-w-[132px] truncate rounded-lg px-2 text-label focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px]"
                  >
                    gpt-5.6-sol
                  </button>
                )}
                onPickProject={setCwd}
                onToggleAnonymous={() => {}}
              />
            )}
          />
        </div>
      </div>
      <div aria-label="Draft project picker probe" data-selected-cwd={cwd ?? ""} />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<DraftProjectPickerAcceptance />);
