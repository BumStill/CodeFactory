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
import { useChatStore } from "./stores/chat";

export type AppView = "workspace" | "resources" | "settings" | "profile" | "control-plane" | "benchmarks" | "evolution";

export default function App() {
  const [view, setView] = useState<AppView>("workspace");
  const [settingsInitialTab, setSettingsInitialTab] = useState<SettingsTab | undefined>();
  const [activeProject, setActiveProject] = useState<string | null>(null);
  const [workspaceTaskLogId, setWorkspaceTaskLogId] = useState<string | null>(null);
  const [evolutionCwd, setEvolutionCwd] = useState<string | null>(null);
  const [evidenceViewerPath, setEvidenceViewerPath] = useState<string | null>(null);
  const loadSettings = useSettingsStore((s) => s.load);
  const settings = useSettingsStore((s) => s.settings);
  const { draftSession, beginQuickDraft } = useChatStore();
  const startupDraftStarted = useRef(false);

  // Workspace is the application shell. Start it with one virtual quick draft:
  // no DB row, scratch directory or history entry exists until first send.
  useEffect(() => {
    if (startupDraftStarted.current) return;
    startupDraftStarted.current = true;
    const draft = draftSession ?? beginQuickDraft();
    setActiveProject(draft.id);
  }, [beginQuickDraft, draftSession]);

  useEffect(() => { loadSettings(); }, [loadSettings]);

  const settingsLoaded = settings != null;
  useEffect(() => {
    if (settingsLoaded) void syncChatGptCatalog();
  }, [settingsLoaded]);

  const openProject = (sessionId: string) => {
    setWorkspaceTaskLogId(null);
    setActiveProject(sessionId);
    setView("workspace");
  };

  const openJobLog = (sessionId: string, taskId: string) => {
    setWorkspaceTaskLogId(taskId);
    setActiveProject(sessionId);
    setView("workspace");
  };

  const openFreshQuickDraft = () => {
    const draft = beginQuickDraft();
    openProject(draft.id);
  };

  const backToWorkspace = () => {
    if (!activeProject) {
      openFreshQuickDraft();
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

      {view === "workspace" && activeProject && (
        <WorkspacePage
          sessionId={activeProject}
          onBackHome={openFreshQuickDraft}
          onOpenSettings={openSettings}
          onOpenUsage={openUsage}
          onOpenSession={openProject}
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
          onOpenSession={openProject}
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
