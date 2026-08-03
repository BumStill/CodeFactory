// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the draft project picker overlay.

import { useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { DraftScopeBar } from "../components/DraftScopeBar";
import type { ProjectGroup } from "../lib/projects";

const projects: ProjectGroup[] = [
  { cwd: "/Users/leo/Projects/CodeFactory", name: "CodeFactory", sessions: [], updatedAt: 2 },
  { cwd: "/Users/leo/Projects/AI foundation", name: "AI foundation", sessions: [], updatedAt: 1 },
];

function DraftProjectPickerAcceptance() {
  const [cwd, setCwd] = useState<string | null>(null);
  return (
    <main aria-label="Draft project picker acceptance" className="flex h-screen flex-col bg-surface-0 p-8 text-gray-200">
      <div className="flex min-h-0 flex-1 items-end justify-center">
        <div
          data-testid="clipped-composer"
          className="h-[70px] w-[640px] overflow-hidden rounded-2xl border border-border/80 bg-surface-2 shadow-lg"
        >
          <DraftScopeBar
            cwd={cwd}
            anonymous={false}
            projects={projects}
            onPickProject={setCwd}
            onToggleAnonymous={() => {}}
          />
          <div className="px-4 pb-4 text-xs text-gray-600">模拟聊天输入框外壳：overflow-hidden</div>
        </div>
      </div>
      <div aria-label="Draft project picker probe" data-selected-cwd={cwd ?? ""} />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<DraftProjectPickerAcceptance />);
