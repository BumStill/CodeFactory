// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { FolderOpen, Plus, Trash2, Settings } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { MessageList } from "../../components/MessageList";
import { MessageInput } from "../../components/MessageInput";
import { ModelPicker } from "../../components/ModelPicker";
import { PermissionDialog } from "../../components/PermissionDialog";
import { useChatStore } from "../../stores/chat";
import { useSettingsStore } from "../../stores/settings";
import {
  formatCostFeedback,
  formatHelpFeedback,
  isKnownSlashCommand,
  usageForSlashCommand,
  type ParsedSlashCommand,
} from "../../stores/slashCommands";

export function ChatPage() {
  const {
    sessions, activeSession, messages, streaming,
    activeModel, inputTokenTotal, outputTokenTotal,
    loadSessions, createSession, selectSession, deleteSession,
    sendMessage, cancelStream, pendingPermission, respondPermission,
    addLocalAssistantMessage, clearVisibleConversation, updateActiveSessionModel,
  } = useChatStore();

  const { settings, load: loadSettings, save: saveSettings } = useSettingsStore();
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    loadSettings();
    loadSessions();
  }, []);

  const handleNewSession = async () => {
    const dir = await open({ directory: true, title: "Choose project folder" });
    if (!dir) return;
    await createSession(dir as string, activeModel);
  };

  const cost = estimateCost(activeModel, inputTokenTotal, outputTokenTotal);
  const fullAccess = settings?.permissions.full_access ?? false;

  const allowWithFullAccess = async () => {
    if (!settings) {
      await respondPermission(true);
      return;
    }
    await saveSettings({
      ...settings,
      permissions: {
        ...settings.permissions,
        full_access: true,
      },
    });
    await respondPermission(true);
  };

  const handleSlashCommand = async (command: ParsedSlashCommand) => {
    if (!isKnownSlashCommand(command.name)) {
      addLocalAssistantMessage(`Unknown slash command: /${command.name}\n\n${formatHelpFeedback()}`);
      return;
    }

    switch (command.name) {
      case "clear":
        clearVisibleConversation();
        return;
      case "model":
        if (!command.args) {
          addLocalAssistantMessage(`Usage: \`${usageForSlashCommand("model")}\``);
          return;
        }
        await updateActiveSessionModel(command.args);
        addLocalAssistantMessage(`Active model set to \`${command.args}\`.`);
        return;
      case "cwd":
        if (!command.args) {
          addLocalAssistantMessage(`Usage: \`${usageForSlashCommand("cwd")}\``);
          return;
        }
        await createSession(command.args, activeModel);
        addLocalAssistantMessage(`Opened project folder:\n\`${command.args}\``);
        return;
      case "cost":
        addLocalAssistantMessage(formatCostFeedback(activeModel, inputTokenTotal, outputTokenTotal));
        return;
      case "help":
        addLocalAssistantMessage(formatHelpFeedback());
        return;
    }
  };

  return (
    <div className="flex h-full bg-surface-0">
      {/* Sidebar */}
      {sidebarOpen && (
        <aside className="w-52 flex-shrink-0 flex flex-col border-r border-border bg-surface-1">
          <div className="flex items-center gap-1 px-3 py-2 border-b border-border">
            <span className="flex-1 text-xs font-semibold text-gray-400 uppercase tracking-wider">Sessions</span>
            <button
              onClick={handleNewSession}
              className="p-1 rounded hover:bg-surface-3 text-gray-500 hover:text-gray-300 transition-colors"
              title="New session"
            >
              <Plus size={14} />
            </button>
          </div>
          <ul className="flex-1 overflow-y-auto py-1">
            {sessions.map((s) => (
              <li key={s.id}>
                <button
                  className={`group w-full flex items-center gap-1 px-3 py-1.5 text-left text-xs transition-colors truncate ${
                    activeSession?.id === s.id
                      ? "bg-surface-3 text-gray-200"
                      : "text-gray-500 hover:bg-surface-2 hover:text-gray-300"
                  }`}
                  onClick={() => selectSession(s.id)}
                >
                  <span className="flex-1 truncate">{s.title}</span>
                  <span
                    className="opacity-0 group-hover:opacity-100 transition-opacity p-0.5 rounded hover:bg-surface-4 text-gray-600 hover:text-red-400"
                    onClick={(e) => { e.stopPropagation(); deleteSession(s.id); }}
                    role="button"
                    title="Delete"
                  >
                    <Trash2 size={10} />
                  </span>
                </button>
              </li>
            ))}
            {sessions.length === 0 && (
              <li className="px-3 py-2 text-xs text-gray-700">No sessions yet</li>
            )}
          </ul>
        </aside>
      )}

      {/* Main */}
      <div className="flex flex-1 flex-col min-w-0">
        {/* Top bar */}
        <header className="flex items-center gap-2 px-3 py-1.5 border-b border-border bg-surface-1 shrink-0">
          <button
            onClick={() => setSidebarOpen((o) => !o)}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="Toggle sidebar"
          >
            <FolderOpen size={14} />
          </button>
          <span className="text-xs text-gray-600 truncate flex-1">
            {activeSession?.cwd ?? "No project open"}
          </span>
          <ModelPicker />
          <button
            onClick={() => setSettingsOpen(true)}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="Settings"
          >
            <Settings size={14} />
          </button>
        </header>

        {/* Messages */}
        <MessageList messages={messages} streaming={streaming} />

        {/* Status bar */}
        {(inputTokenTotal > 0 || outputTokenTotal > 0) && (
          <div className="flex items-center gap-3 px-4 py-1 border-t border-border text-xs text-gray-700 bg-surface-1 shrink-0 select-none">
            <span>↑ {inputTokenTotal.toLocaleString()} tokens</span>
            <span>↓ {outputTokenTotal.toLocaleString()} tokens</span>
            {cost != null && <span>≈ ${cost}</span>}
          </div>
        )}

        {/* Input */}
        <MessageInput
          onSend={sendMessage}
          onCommand={handleSlashCommand}
          onCancel={cancelStream}
          streaming={streaming}
          disabled={!activeSession}
        />
      </div>

      {/* Settings modal (stub) */}
      {settingsOpen && (
        <SettingsModal onClose={() => setSettingsOpen(false)} />
      )}

      {pendingPermission && (
        <PermissionDialog
          request={pendingPermission}
          fullAccess={fullAccess}
          onAllow={() => respondPermission(true)}
          onDeny={() => respondPermission(false)}
          onAllowFullAccess={allowWithFullAccess}
        />
      )}
    </div>
  );
}

function SettingsModal({ onClose }: { onClose: () => void }) {
  const { settings, save, saveApiKey, getApiKey } = useSettingsStore();
  const [key, setKey] = useState("");
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (!settings) return;
    const ep = settings.endpoints[settings.default_endpoint];
    if (ep?.key_ref) {
      getApiKey(ep.key_ref).then((k) => setKey(k ?? ""));
    }
  }, [settings]);

  const handleSave = async () => {
    if (!settings) return;
    const ep = settings.endpoints[settings.default_endpoint];
    if (!ep?.key_ref) return;
    await saveApiKey(ep.key_ref, key);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };

  const setFullAccess = async (fullAccess: boolean) => {
    if (!settings) return;
    await save({
      ...settings,
      permissions: {
        ...settings.permissions,
        full_access: fullAccess,
      },
    });
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div
        className="w-96 rounded-xl border border-border bg-surface-2 shadow-2xl p-5 space-y-4"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-gray-200">Settings</h2>

        <div className="space-y-2">
          <label className="block text-xs text-gray-500">OpenRouter API Key</label>
          <input
            type="password"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder="sk-or-..."
            className="w-full bg-surface-3 border border-border rounded px-3 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>

        <label className="flex items-start gap-3 rounded-lg border border-border bg-surface-1 px-3 py-2">
          <input
            type="checkbox"
            checked={settings?.permissions.full_access ?? false}
            onChange={(e) => setFullAccess(e.target.checked)}
            className="mt-0.5"
          />
          <span className="min-w-0">
            <span className="block text-xs font-medium text-gray-200">Full access mode</span>
            <span className="block text-xs leading-5 text-gray-500">
              Bypass configured allow/ask/deny prompts for future tool calls. Use only in trusted projects.
            </span>
          </span>
        </label>

        <div className="flex justify-end gap-2">
          <button onClick={onClose} className="px-3 py-1.5 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors">
            Cancel
          </button>
          <button
            onClick={handleSave}
            className="px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white transition-colors"
          >
            {saved ? "Saved ✓" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}

function estimateCost(model: string, inputTok: number, outputTok: number): string | null {
  if (inputTok === 0 && outputTok === 0) return null;
  // Very rough: $3/M input, $15/M output for Opus-class models
  const isOpus = model.includes("opus") || model.includes("gpt-4");
  const inputPrice = isOpus ? 3 : 0.5;
  const outputPrice = isOpus ? 15 : 1.5;
  const cost = (inputTok / 1_000_000) * inputPrice + (outputTok / 1_000_000) * outputPrice;
  return cost.toFixed(4);
}
