// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the draft project picker overlay and the
// CF-MSH-R2 model-picker/default-model agreement.

import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { DraftScopeBar } from "../components/DraftScopeBar";
import { MessageInput } from "../components/MessageInput";
import { ModelPicker } from "../components/ModelPicker";
import { useChatStore } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";
import type { ProjectGroup } from "../lib/projects";

// Synthetic catalogue. "gpt-6-luna" stands in for the stale cached selection
// that the rejected M51 behaviour carried into a brand-new session.
const MODELS = [
  { id: "gpt-6-luna", name: "gpt-6-luna", context_length: 272000 },
  { id: "gpt-6.1-sol", name: "gpt-6.1-sol", context_length: 272000 },
  { id: "gpt-6.1-flash", name: "gpt-6.1-flash", context_length: 272000 },
];

function seedDefault(model: string) {
  useSettingsStore.setState({
    settings: {
      default_endpoint: "acceptance",
      default_model: model,
      endpoints: {
        acceptance: {
          base_url: "https://acceptance.invalid",
          api_key: "",
          api_style: "openai",
          custom_models: MODELS.map((candidate) => ({ id: candidate.id, name: candidate.name })),
          active_model: model,
        },
      },
    } as never,
    load: async () => {},
  });
}

useChatStore.setState({
  models: MODELS,
  // Deliberately stale: the settings default below is a different model, so a
  // draft that reused this cached value would show and use the wrong model.
  activeModel: "gpt-6-luna",
  activeSession: null,
  draftSession: null,
  loadModels: async () => {},
});
seedDefault("gpt-6.1-sol");

interface ModelSelectionAcceptance {
  seedDefault: (model: string) => void;
  beginDraft: () => void;
}

declare global {
  interface Window {
    __modelSelectionAcceptance?: ModelSelectionAcceptance;
  }
}

const projects: ProjectGroup[] = [
  { cwd: "/Users/leo/Projects/CodeFactory", name: "CodeFactory", sessions: [], updatedAt: 2 },
  { cwd: "/Users/leo/Projects/AI foundation", name: "AI foundation", sessions: [], updatedAt: 1 },
];

function ModelSelectionProbe() {
  const activeModel = useChatStore((state) => state.activeModel);
  const draftModel = useChatStore((state) => state.draftSession?.modelId ?? "");
  return (
    <div
      aria-label="Model selection probe"
      data-active-model={activeModel}
      data-draft-model={draftModel}
    />
  );
}

function DraftProjectPickerAcceptance() {
  const [cwd, setCwd] = useState<string | null>(null);

  useEffect(() => {
    window.__modelSelectionAcceptance = {
      seedDefault,
      beginDraft: () => {
        useChatStore.getState().beginDraft();
      },
    };
    return () => {
      delete window.__modelSelectionAcceptance;
    };
  }, []);

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
      <ModelSelectionProbe />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<DraftProjectPickerAcceptance />);
