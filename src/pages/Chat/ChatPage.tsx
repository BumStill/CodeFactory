// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import { FolderOpen, Plus, Trash2, Settings, TerminalSquare, BookOpen, Puzzle } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { MessageList } from "../../components/MessageList";
import { MessageInput } from "../../components/MessageInput";
import { ModelPicker } from "../../components/ModelPicker";
import { PermissionDialog } from "../../components/PermissionDialog";
import { FileTree } from "../../components/FileTree";
import { GitStatusBar } from "../../components/GitStatusBar";
import { GitChangesPanel } from "../../components/GitChangesPanel";
import { GitHistoryPanel } from "../../components/GitHistoryPanel";
import { RemoteGitPanel } from "../../components/RemoteGitPanel";
import { useChatStore } from "../../stores/chat";
import { useSettingsStore } from "../../stores/settings";
import { useGitStore } from "../../stores/git";
import {
  formatCostFeedback,
  formatHelpFeedback,
  isKnownSlashCommand,
  usageForSlashCommand,
  type ParsedSlashCommand,
} from "../../stores/slashCommands";
import { useSkillsStore } from "../../stores/skills";
import { useGitRemoteStore } from "../../stores/gitRemote";
import { invoke } from "../../lib/tauri";
import type { GitRemoteConfig, GitProvider } from "../../lib/tauri";
import Terminal from "../../components/Terminal";

interface ChatPageProps {
  onOpenSpecs?: () => void;
  onOpenSkills?: () => void;
}

export function ChatPage({ onOpenSpecs, onOpenSkills }: ChatPageProps) {
  const {
    sessions, activeSession, messages, streaming,
    activeModel, inputTokenTotal, outputTokenTotal,
    loadSessions, createSession, selectSession, deleteSession,
    sendMessage, cancelStream, pendingPermission, respondPermission,
    addLocalAssistantMessage, clearVisibleConversation, updateActiveSessionModel,
  } = useChatStore();

  const { settings, load: loadSettings, save: saveSettings } = useSettingsStore();
  const { skills, loadSkills } = useSkillsStore();
  const { status: gitStatus } = useGitStore();
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [terminalOpen, setTerminalOpen] = useState(false);
  const [pendingInsert, setPendingInsert] = useState<string | undefined>(undefined);
  const [gitChangesOpen, setGitChangesOpen] = useState(false);
  const [gitHistoryOpen, setGitHistoryOpen] = useState(false);
  const [remoteGitOpen, setRemoteGitOpen] = useState(false);
  const [skillSlashCommands, setSkillSlashCommands] = useState<Array<{ name: string; description: string; template: string }>>([]);
  // Stable terminal session id for this page instance.
  const terminalId = useRef(`term-${Math.random().toString(36).slice(2)}`).current;

  useEffect(() => {
    loadSettings();
    loadSessions();
    loadSkills();
  }, []);

  // Reload skill slash commands whenever skills change
  useEffect(() => {
    invoke<Array<{ name: string; description: string; template: string }>>("list_slash_commands")
      .then((cmds) => setSkillSlashCommands(cmds))
      .catch(() => {});
  }, [skills]);

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
    // Check skill slash commands first
    const skillCmd = skillSlashCommands.find((c) => c.name === command.name);
    if (skillCmd) {
      const expanded = skillCmd.template.replace("{input}", command.args || "");
      await sendMessage(expanded);
      return;
    }

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
          {activeSession?.cwd && (
            <FileTree
              cwd={activeSession.cwd}
              onSelectFile={(path) => setPendingInsert(path)}
            />
          )}
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
            onClick={() => setTerminalOpen((o) => !o)}
            className={`p-1 rounded transition-colors ${
              terminalOpen
                ? "text-accent bg-surface-3"
                : "text-gray-600 hover:text-gray-300 hover:bg-surface-3"
            }`}
            title="Toggle terminal"
          >
            <TerminalSquare size={14} />
          </button>
          {onOpenSpecs && (
            <button
              onClick={onOpenSpecs}
              className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
              title="Spec Workbench"
            >
              <BookOpen size={14} />
            </button>
          )}
          {onOpenSkills && (
            <button
              onClick={onOpenSkills}
              className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
              title="Skills"
            >
              <Puzzle size={14} />
            </button>
          )}
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

        {/* Git status bar */}
        <GitStatusBar
          cwd={activeSession?.cwd ?? null}
          onOpenChanges={() => {
            setGitHistoryOpen(false);
            setRemoteGitOpen(false);
            setGitChangesOpen((v) => !v);
          }}
          onOpenHistory={() => {
            setGitChangesOpen(false);
            setRemoteGitOpen(false);
            setGitHistoryOpen((v) => !v);
          }}
          onOpenRemote={() => {
            setGitChangesOpen(false);
            setGitHistoryOpen(false);
            setRemoteGitOpen((v) => !v);
          }}
        />

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
          pendingInsert={pendingInsert}
          onInsertConsumed={() => setPendingInsert(undefined)}
          skillSlashCommands={skillSlashCommands}
        />

        {/* Terminal panel */}
        {terminalOpen && (
          <div
            className="border-t border-border bg-[#1e1e1e] shrink-0"
            style={{ height: 300 }}
          >
            <Terminal id={terminalId} />
          </div>
        )}
      </div>

      {/* Git panels (right-side overlays) */}
      {gitChangesOpen && <GitChangesPanel onClose={() => setGitChangesOpen(false)} />}
      {gitHistoryOpen && <GitHistoryPanel onClose={() => setGitHistoryOpen(false)} />}
      {remoteGitOpen && (
        <RemoteGitPanel
          cwd={activeSession?.cwd ?? null}
          currentBranch={gitStatus?.branch ?? ""}
          onClose={() => setRemoteGitOpen(false)}
        />
      )}

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
  const [apiStyle, setApiStyle] = useState<"openai" | "anthropic">("openai");
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (!settings) return;
    const ep = settings.endpoints[settings.default_endpoint];
    if (ep?.key_ref) {
      getApiKey(ep.key_ref).then((k) => setKey(k ?? ""));
    }
    setApiStyle(ep?.api_style ?? "openai");
  }, [settings]);

  const handleSave = async () => {
    if (!settings) return;
    const ep = settings.endpoints[settings.default_endpoint];
    if (!ep) return;
    if (ep.key_ref) {
      await saveApiKey(ep.key_ref, key);
    }
    await save({
      ...settings,
      endpoints: {
        ...settings.endpoints,
        [settings.default_endpoint]: {
          ...ep,
          api_style: apiStyle,
        },
      },
    });
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
        className="w-[520px] max-h-[80vh] overflow-y-auto rounded-xl border border-border bg-surface-2 shadow-2xl p-5 space-y-4"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-gray-200">Settings</h2>

        <div className="space-y-2">
          <label className="block text-xs text-gray-500">API Key</label>
          <input
            type="password"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder="sk-..."
            className="w-full bg-surface-3 border border-border rounded px-3 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>

        <div className="space-y-2">
          <span className="block text-xs text-gray-500">API Style</span>
          <div className="flex gap-4">
            <label className="flex items-center gap-1.5 cursor-pointer">
              <input
                type="radio"
                name="api_style"
                value="openai"
                checked={apiStyle === "openai"}
                onChange={() => setApiStyle("openai")}
              />
              <span className="text-xs text-gray-300">OpenAI Compatible</span>
            </label>
            <label className="flex items-center gap-1.5 cursor-pointer">
              <input
                type="radio"
                name="api_style"
                value="anthropic"
                checked={apiStyle === "anthropic"}
                onChange={() => setApiStyle("anthropic")}
              />
              <span className="text-xs text-gray-300">Anthropic Messages API</span>
            </label>
          </div>
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

        <HooksSection />
        <RemotesSection />
      </div>
    </div>
  );
}

// ── Hooks Section ─────────────────────────────────────────────────────────────

type HookActionType = "log_to_file" | "run_command" | "emit_event" | "auto_git_commit";

interface HookConfig {
  id: string;
  name: string;
  event: string;
  action: { type: HookActionType; path?: string; command?: string; cwd?: string; event_name?: string; message_template?: string };
  enabled: boolean;
  filter: string | null;
}

function HooksSection() {
  const [hooks, setHooks] = useState<HookConfig[]>([]);
  const [addOpen, setAddOpen] = useState(false);
  const [testResult, setTestResult] = useState<{ id: string; result: string } | null>(null);

  const loadHooks = async () => {
    try {
      const h = await invoke<HookConfig[]>("list_hooks");
      setHooks(h);
    } catch {}
  };

  useEffect(() => { loadHooks(); }, []);

  const handleToggle = async (hook: HookConfig) => {
    await invoke("update_hook", { id: hook.id, config: { ...hook, enabled: !hook.enabled } });
    await loadHooks();
  };

  const handleDelete = async (id: string) => {
    await invoke("delete_hook", { id });
    await loadHooks();
  };

  const handleTest = async (id: string) => {
    try {
      const result = await invoke<string>("test_hook", { id });
      setTestResult({ id, result });
    } catch (e) {
      setTestResult({ id, result: String(e) });
    }
  };

  return (
    <div className="border-t border-border pt-4 space-y-3">
      <div className="flex items-center gap-2">
        <span className="text-xs font-semibold text-gray-400 uppercase tracking-wider flex-1">Hooks</span>
        <button
          onClick={() => setAddOpen(true)}
          className="px-2 py-1 rounded text-xs bg-surface-3 text-gray-400 hover:text-gray-200 transition-colors"
        >
          + Add Hook
        </button>
      </div>

      {hooks.length === 0 && (
        <p className="text-xs text-gray-700">No hooks configured.</p>
      )}

      {hooks.map((hook) => (
        <div key={hook.id} className="rounded border border-border bg-surface-1 px-3 py-2 space-y-1">
          <div className="flex items-center gap-2">
            <span className="flex-1 text-xs text-gray-200 font-medium truncate">{hook.name}</span>
            <span className="text-[10px] text-gray-600 bg-surface-3 px-1.5 py-0.5 rounded">{hook.event}</span>
            <button
              onClick={() => handleToggle(hook)}
              className={`text-[10px] px-1.5 py-0.5 rounded transition-colors ${
                hook.enabled ? "bg-accent/20 text-accent" : "bg-surface-3 text-gray-600"
              }`}
            >
              {hook.enabled ? "on" : "off"}
            </button>
            <button
              onClick={() => handleTest(hook.id)}
              className="text-[10px] text-gray-600 hover:text-gray-300 px-1.5 py-0.5 rounded hover:bg-surface-3 transition-colors"
            >
              test
            </button>
            <button
              onClick={() => handleDelete(hook.id)}
              className="text-[10px] text-red-700 hover:text-red-400 px-1.5 py-0.5 rounded hover:bg-surface-3 transition-colors"
            >
              del
            </button>
          </div>
          <div className="text-[10px] text-gray-600 font-mono truncate">
            {hook.action.type}: {hook.action.path ?? hook.action.command ?? hook.action.event_name ?? hook.action.message_template ?? ""}
          </div>
          {testResult?.id === hook.id && (
            <pre className="text-[10px] text-gray-400 bg-surface-3 rounded p-1.5 whitespace-pre-wrap max-h-20 overflow-y-auto">
              {testResult.result}
            </pre>
          )}
        </div>
      ))}

      {addOpen && <AddHookForm onAdded={() => { loadHooks(); setAddOpen(false); }} onCancel={() => setAddOpen(false)} />}
    </div>
  );
}

function AddHookForm({ onAdded, onCancel }: { onAdded: () => void; onCancel: () => void }) {
  const [name, setName] = useState("");
  const [event, setEvent] = useState("post_tool");
  const [actionType, setActionType] = useState<HookActionType>("log_to_file");
  const [actionParam, setActionParam] = useState("");
  const [filter, setFilter] = useState("");
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const EVENT_OPTIONS = ["pre_tool", "post_tool", "pre_task", "post_task", "session_start", "session_end", "spec_approved", "verification_failed"];
  const ACTION_OPTIONS: { value: HookActionType; label: string; placeholder: string }[] = [
    { value: "log_to_file", label: "Log to file", placeholder: "C:\\logs\\codefactory.jsonl" },
    { value: "run_command", label: "Run command", placeholder: "echo hook fired" },
    { value: "emit_event", label: "Emit Tauri event", placeholder: "my-hook-event" },
    { value: "auto_git_commit", label: "Auto git commit (post_task only)", placeholder: "chore: {task_title}" },
  ];

  const buildAction = () => {
    switch (actionType) {
      case "log_to_file": return { type: "log_to_file" as const, path: actionParam };
      case "run_command": return { type: "run_command" as const, command: actionParam, cwd: null };
      case "emit_event": return { type: "emit_event" as const, event_name: actionParam };
      case "auto_git_commit": return { type: "auto_git_commit" as const, message_template: actionParam };
    }
  };

  const handleSave = async () => {
    if (!name.trim() || !actionParam.trim()) {
      setErr("Name and action param are required.");
      return;
    }
    setSaving(true);
    setErr(null);
    try {
      await invoke("add_hook", {
        config: {
          id: `hook-${Date.now()}`,
          name: name.trim(),
          event,
          action: buildAction(),
          enabled: true,
          filter: filter.trim() || null,
        }
      });
      onAdded();
    } catch (e) {
      setErr(String(e));
    } finally {
      setSaving(false);
    }
  };

  const currentAction = ACTION_OPTIONS.find((a) => a.value === actionType);

  return (
    <div className="rounded border border-accent/30 bg-surface-1 p-3 space-y-2">
      <p className="text-xs font-medium text-gray-300">New Hook</p>
      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block text-[10px] text-gray-600 mb-0.5">Name</label>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="My Hook"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>
        <div>
          <label className="block text-[10px] text-gray-600 mb-0.5">Event</label>
          <select
            value={event}
            onChange={(e) => setEvent(e.target.value)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none"
          >
            {EVENT_OPTIONS.map((ev) => <option key={ev} value={ev}>{ev}</option>)}
          </select>
        </div>
        <div>
          <label className="block text-[10px] text-gray-600 mb-0.5">Action type</label>
          <select
            value={actionType}
            onChange={(e) => setActionType(e.target.value as HookActionType)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none"
          >
            {ACTION_OPTIONS.map((a) => <option key={a.value} value={a.value}>{a.label}</option>)}
          </select>
        </div>
        <div>
          <label className="block text-[10px] text-gray-600 mb-0.5">Filter (optional)</label>
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="e.g. bash"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>
      </div>
      <div>
        <label className="block text-[10px] text-gray-600 mb-0.5">
          {currentAction?.label ?? "Param"}
        </label>
        <input
          value={actionParam}
          onChange={(e) => setActionParam(e.target.value)}
          placeholder={currentAction?.placeholder ?? ""}
          className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
        />
      </div>
      {err && <p className="text-xs text-red-400">{err}</p>}
      <div className="flex justify-end gap-2">
        <button onClick={onCancel} className="px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors">
          Cancel
        </button>
        <button
          onClick={handleSave}
          disabled={saving}
          className="px-2 py-1 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
        >
          {saving ? "Adding..." : "Add Hook"}
        </button>
      </div>
    </div>
  );
}

// ── Remotes Section ───────────────────────────────────────────────────────────

function RemotesSection() {
  const { remotes, loadRemotes, addRemote, deleteRemote, testRemote } =
    useGitRemoteStore();

  const [addOpen, setAddOpen] = useState(false);
  const [testResults, setTestResults] = useState<Record<string, string>>({});
  const [testing, setTesting] = useState<string | null>(null);

  useEffect(() => { loadRemotes(); }, [loadRemotes]);

  const handleTest = async (id: string) => {
    setTesting(id);
    try {
      const username = await testRemote(id);
      setTestResults((r) => ({ ...r, [id]: `OK: @${username}` }));
    } catch (e) {
      setTestResults((r) => ({ ...r, [id]: `Error: ${String(e)}` }));
    } finally {
      setTesting(null);
    }
  };

  return (
    <div className="border-t border-border pt-4 space-y-3">
      <div className="flex items-center gap-2">
        <span className="text-xs font-semibold text-gray-400 uppercase tracking-wider flex-1">
          Remotes (GitHub / GitLab)
        </span>
        <button
          onClick={() => setAddOpen(true)}
          className="px-2 py-1 rounded text-xs bg-surface-3 text-gray-400 hover:text-gray-200 transition-colors"
        >
          + Add Remote
        </button>
      </div>

      {remotes.length === 0 && (
        <p className="text-xs text-gray-700">No remotes configured.</p>
      )}

      {remotes.map((remote: GitRemoteConfig) => (
        <div key={remote.id} className="rounded border border-border bg-surface-1 px-3 py-2 space-y-1">
          <div className="flex items-center gap-2">
            <span
              className={`text-[9px] px-1.5 py-0.5 rounded font-medium ${
                remote.provider === "github"
                  ? "bg-gray-700 text-gray-200"
                  : "bg-orange-900 text-orange-200"
              }`}
            >
              {remote.provider}
            </span>
            <span className="flex-1 text-xs text-gray-200 font-medium truncate">{remote.name}</span>
            {remote.default_repo && (
              <span className="text-[10px] text-gray-600 font-mono truncate max-w-[120px]">
                {remote.default_repo}
              </span>
            )}
            <button
              onClick={() => handleTest(remote.id)}
              disabled={testing === remote.id}
              className="text-[10px] text-gray-600 hover:text-gray-300 px-1.5 py-0.5 rounded hover:bg-surface-3 transition-colors disabled:opacity-50"
            >
              {testing === remote.id ? "Testing..." : "Test"}
            </button>
            <button
              onClick={() => deleteRemote(remote.id)}
              className="text-[10px] text-red-700 hover:text-red-400 px-1.5 py-0.5 rounded hover:bg-surface-3 transition-colors"
            >
              del
            </button>
          </div>
          {testResults[remote.id] && (
            <div className={`text-[10px] px-1 py-0.5 rounded ${
              testResults[remote.id].startsWith("OK")
                ? "text-green-400"
                : "text-red-400"
            }`}>
              {testResults[remote.id]}
            </div>
          )}
        </div>
      ))}

      {addOpen && (
        <AddRemoteForm
          onAdded={() => { loadRemotes(); setAddOpen(false); }}
          onCancel={() => setAddOpen(false)}
          addRemote={addRemote}
        />
      )}
    </div>
  );
}

function AddRemoteForm({
  onAdded,
  onCancel,
  addRemote,
}: {
  onAdded: () => void;
  onCancel: () => void;
  addRemote: (config: GitRemoteConfig) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [provider, setProvider] = useState<GitProvider>("github");
  const [baseUrl, setBaseUrl] = useState("https://api.github.com");
  const [token, setToken] = useState("");
  const [defaultRepo, setDefaultRepo] = useState("");
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  // Auto-fill base URL when provider changes
  const handleProviderChange = (p: GitProvider) => {
    setProvider(p);
    if (p === "github") setBaseUrl("https://api.github.com");
    else if (p === "gitlab") setBaseUrl("https://gitlab.com/api/v4");
  };

  const handleSave = async () => {
    if (!name.trim() || !token.trim()) {
      setErr("Name and token are required.");
      return;
    }
    setSaving(true);
    setErr(null);
    try {
      await addRemote({
        id: "",
        name: name.trim(),
        provider,
        base_url: baseUrl.trim(),
        token: token.trim(),
        default_repo: defaultRepo.trim() || null,
      });
      onAdded();
    } catch (e) {
      setErr(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="rounded border border-accent/30 bg-surface-1 p-3 space-y-2">
      <p className="text-xs font-medium text-gray-300">New Remote</p>
      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block text-[10px] text-gray-600 mb-0.5">Name</label>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="My GitHub"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>
        <div>
          <label className="block text-[10px] text-gray-600 mb-0.5">Provider</label>
          <select
            value={provider}
            onChange={(e) => handleProviderChange(e.target.value as GitProvider)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none"
          >
            <option value="github">GitHub</option>
            <option value="gitlab">GitLab</option>
          </select>
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-600 mb-0.5">Base URL</label>
          <input
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none focus:border-accent/50"
          />
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-600 mb-0.5">Personal Access Token</label>
          <input
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="ghp_..."
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-600 mb-0.5">Default Repo (optional)</label>
          <input
            value={defaultRepo}
            onChange={(e) => setDefaultRepo(e.target.value)}
            placeholder="owner/repo"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>
      </div>
      {err && <p className="text-xs text-red-400">{err}</p>}
      <div className="flex justify-end gap-2">
        <button
          onClick={onCancel}
          className="px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
        >
          Cancel
        </button>
        <button
          onClick={handleSave}
          disabled={saving}
          className="px-2 py-1 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
        >
          {saving ? "Adding..." : "Add Remote"}
        </button>
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
