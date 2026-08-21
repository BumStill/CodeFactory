// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the draft project picker overlay.

import { useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { DraftScopeBar } from "../components/DraftScopeBar";
import { MessageInput } from "../components/MessageInput";
import { ModelPicker } from "../components/ModelPicker";
import { useChatStore } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";
import type { ProjectGroup } from "../lib/projects";

useChatStore.setState({
  models: [{ id: "gpt-5.6-sol", name: "gpt-5.6-sol", context_length: 272000 }],
  activeModel: "gpt-5.6-sol",
  activeSession: null,
  loadModels: async () => {},
});
useSettingsStore.setState({
  settings: {
    default_endpoint: "acceptance",
    default_model: "gpt-5.6-sol",
    endpoints: {
      acceptance: {
        base_url: "https://acceptance.invalid",
        api_key: "",
        api_style: "openai",
        custom_models: [{ id: "gpt-5.6-sol", name: "gpt-5.6-sol" }],
        active_model: "gpt-5.6-sol",
      },
    },
  } as never,
  load: async () => {},
});

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
                modelPicker={<ModelPicker portal />}
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
