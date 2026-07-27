// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import { WorkspacePage } from "./pages/Workspace/WorkspacePage";
import { ResourcesPage } from "./pages/Resources/ResourcesPage";
import { ControlPlanePage } from "./pages/ControlPlane/ControlPlanePage";
import { BenchmarksPage } from "./pages/Benchmarks/BenchmarksPage";
import { EvolutionWorkbenchPage } from "./pages/Evolution/EvolutionWorkbenchPage";
import { SettingsPage, type SettingsTab } from "./pages/Settings/SettingsPage";
import { ProfilePage } from "./pages/Profile/ProfilePage";
import { ToastContainer } from "./components/Toast";
import { EvidenceViewer } from "./components/EvidenceViewer";
import { UpdaterBanner } from "./components/UpdaterBanner";
import { OnboardingWizard } from "./components/OnboardingWizard";
import { useSettingsStore } from "./stores/settings";
import { syncChatGptCatalog } from "./stores/chatgptCatalog";
import { useChatStore, openSessionId } from "./stores/chat";

export type AppView = "workspace" | "resources" | "settings" | "profile" | "control-plane" | "benchmarks" | "evolution";

export default function App() {
  const [view, setView] = useState<AppView>("workspace");
  const [settingsInitialTab, setSettingsInitialTab] = useState<SettingsTab | undefined>();
  const [workspaceTaskLogId, setWorkspaceTaskLogId] = useState<string | null>(null);
  const [evolutionCwd, setEvolutionCwd] = useState<string | null>(null);
  const [evidenceViewerPath, setEvidenceViewerPath] = useState<string | null>(null);
  const loadSettings = useSettingsStore((s) => s.load);
  const settings = useSettingsStore((s) => s.settings);
  const { beginDraft, selectSession } = useChatStore();
  // Which conversation is open is DERIVED from the store, never copied into
  // React state. The shell used to hold its own id, which could drift from the
  // store — the chat pane showed one conversation while tasks, git and
  // interjections addressed another (and a stale id produced an unhandled
  // rejection that silently kept the old conversation on screen).
  const openSession = useChatStore(openSessionId);
  const startupDraftStarted = useRef(false);

  // Workspace is the application shell. Start it with one virtual draft:
  // no DB row, scratch directory or history entry exists until first send.
  useEffect(() => {
    if (startupDraftStarted.current) return;
    startupDraftStarted.current = true;
    if (!useChatStore.getState().draftSession && !useChatStore.getState().activeSession) {
      beginDraft();
    }
  }, [beginDraft]);

  useEffect(() => { loadSettings(); }, [loadSettings]);

  const settingsLoaded = settings != null;
  useEffect(() => {
    if (settingsLoaded) void syncChatGptCatalog();
  }, [settingsLoaded]);

  // Opening an existing conversation is one act with one entry point: it
  // resolves the session in the store, and the workspace follows because its
  // id is derived from that store.
  const openExistingSession = (sessionId: string) => {
    setWorkspaceTaskLogId(null);
    void selectSession(sessionId);
    setView("workspace");
  };

  const openJobLog = (sessionId: string, taskId: string) => {
    setWorkspaceTaskLogId(taskId);
    void selectSession(sessionId);
    setView("workspace");
  };

  /** Start a blank conversation, optionally scoped to a project directory. */
  const startNewConversation = (cwd?: string | null) => {
    setWorkspaceTaskLogId(null);
    beginDraft({ cwd: cwd ?? null });
    setView("workspace");
  };

  const backToWorkspace = () => {
    if (!openSession) {
      startNewConversation(null);
      return;
    }
    setView("workspace");
  };

  const openEvolution = (cwd?: string) => {
    setEvolutionCwd(cwd ?? null);
    setView("evolution");
  };

  const openSettings = (tab?: SettingsTab) => {
    // Keep this callback safe when legacy embedders pass it directly to
    // onClick and React supplies a SyntheticEvent instead of a tab id.
    setSettingsInitialTab(typeof tab === "string" ? tab : "capabilities");
    setView("settings");
  };

  const openUsage = () => {
    setSettingsInitialTab("usage");
    setView("settings");
  };

  return (
    <>
      <UpdaterBanner />

      {view === "workspace" && openSession && (
        <WorkspacePage
          sessionId={openSession}
          onNewConversation={startNewConversation}
          onOpenSettings={openSettings}
          onOpenUsage={openUsage}
          onOpenSession={openExistingSession}
          initialTaskLogId={workspaceTaskLogId}
        />
      )}

      {view === "resources" && <ResourcesPage onBack={backToWorkspace} />}
      {view === "control-plane" && <ControlPlanePage onBack={backToWorkspace} />}
      {view === "benchmarks" && <BenchmarksPage onBack={backToWorkspace} />}
      {view === "evolution" && <EvolutionWorkbenchPage onBack={backToWorkspace} initialCwd={evolutionCwd} />}
      {view === "profile" && <ProfilePage onBack={backToWorkspace} onOpenEvolution={openEvolution} />}
      {view === "settings" && (
        <SettingsPage
          onBack={backToWorkspace}
          initialTab={settingsInitialTab}
          onOpenSession={openExistingSession}
          onOpenJobLog={openJobLog}
          onOpenResources={() => setView("resources")}
          onOpenControlPlane={() => setView("control-plane")}
          onOpenBenchmarks={() => setView("benchmarks")}
          onOpenProfile={() => setView("profile")}
          onOpenEvolution={() => openEvolution()}
        />
      )}

      {settings && !settings.onboarded && (
        <OnboardingWizard
          modelReady={Object.values(settings.endpoints ?? {}).length > 0}
          ceiling={settings.delivery_ceiling ?? "pr_only"}
          onCeilingChange={(ceiling) =>
            void useSettingsStore.getState().save({ ...settings, delivery_ceiling: ceiling })
          }
          onDone={() =>
            void useSettingsStore.getState().save({ ...settings, onboarded: true })
          }
        />
      )}
      <ToastContainer onViewPack={(path) => setEvidenceViewerPath(path)} />

      {evidenceViewerPath && (
        <EvidenceViewer
          packPath={evidenceViewerPath}
          onClose={() => setEvidenceViewerPath(null)}
        />
      )}
    </>
  );
}
