// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import { FolderOpen, Plus, Trash2, Settings, TerminalSquare, BookOpen, Puzzle } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { MessageList } from "../../components/MessageList";
import { MessageInput } from "../../components/MessageInput";
import { ModelPicker } from "../../components/ModelPicker";
import { UpdateStatusPill } from "../../components/UpdateStatusPill";
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
import { invoke } from "../../lib/tauri";
import Terminal from "../../components/Terminal";
import { ContextUsageBar } from "../../components/ContextUsageBar";

interface ChatPageProps {
  onOpenSpecs?: () => void;
  onOpenSkills?: () => void;
  onOpenSettings?: () => void;
}

export function ChatPage({ onOpenSpecs, onOpenSkills, onOpenSettings }: ChatPageProps) {
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
          <UpdateStatusPill />
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
            onClick={onOpenSettings}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="Settings"
          >
            <Settings size={14} />
          </button>
        </header>

        {/* Messages */}
        <MessageList
          messages={messages}
          streaming={streaming}
          onUsePrompt={(text) => setPendingInsert(text)}
        />

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

        {/* Combined token totals + context-window usage bar */}
        <ContextUsageBar sessionId={activeSession?.id} />

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

