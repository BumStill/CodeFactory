// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { HomePage } from "./pages/Home/HomePage";
import { WorkspacePage } from "./pages/Workspace/WorkspacePage";
import { SkillsPage } from "./pages/Skills/SkillsPage";
import { ControlPlanePage } from "./pages/ControlPlane/ControlPlanePage";
import { BenchmarksPage } from "./pages/Benchmarks/BenchmarksPage";
import { SettingsPage } from "./pages/Settings/SettingsPage";
import { ProfilePage } from "./pages/Profile/ProfilePage";
import { ToastContainer } from "./components/Toast";
import { EvidenceViewer } from "./components/EvidenceViewer";
import { UpdaterBanner } from "./components/UpdaterBanner";
import { OnboardingOverlay } from "./components/OnboardingOverlay";
import { useSettingsStore } from "./stores/settings";
import { useChatStore } from "./stores/chat";
import { invoke } from "./lib/tauri";
import type { Session } from "./lib/tauri";

export type AppView = "home" | "workspace" | "specs" | "skills" | "settings" | "profile" | "control-plane" | "benchmarks";

export default function App() {
  const [view, setView] = useState<AppView>("home");
  const [activeProject, setActiveProject] = useState<string | null>(null);
  const [evidenceViewerPath, setEvidenceViewerPath] = useState<string | null>(null);
  const loadSettings = useSettingsStore((s) => s.load);
  const settings = useSettingsStore((s) => s.settings);
  const { activeModel, createSession } = useChatStore();
  const [onboardingDismissed, setOnboardingDismissed] = useState(false);

  // Load settings once at app start so the theme + font apply before first
  // paint of any page (otherwise pages flash dark before switching to light).
  useEffect(() => { loadSettings(); }, []);

  const openProject = (sessionId: string) => {
    setActiveProject(sessionId);
    setView("workspace");
  };

  const backToHome = () => {
    setView("home");
  };

  // Onboarding gate: show overlay when settings loaded AND user hasn't
  // completed/skipped it yet. Local `onboardingDismissed` prevents the
  // overlay flashing back after the user clicks an action during the
  // brief window where settings.onboarded hasn't round-tripped to disk.
  const needsOnboarding = settings != null && !settings.onboarded && !onboardingDismissed;

  // Onboarding actions that touch chat store / dialogs, lifted to App so
  // OnboardingOverlay stays pure.
  const onboardingNewProject = async () => {
    setOnboardingDismissed(true);
    const dir = await openDialog({ directory: true, title: "选择项目目录" });
    if (!dir) return;
    const session = await createSession(dir as string, activeModel);
    if (session) openProject(session.id);
  };
  const onboardingQuickTask = async () => {
    setOnboardingDismissed(true);
    try {
      const session = await invoke<Session>("get_or_create_quick_session", {
        modelId: activeModel,
      });
      openProject(session.id);
    } catch {
      // fall through to Home — user can retry from there
    }
  };
  const onboardingProfile = () => {
    setOnboardingDismissed(true);
    setView("profile");
  };

  return (
    <>
      <UpdaterBanner />

      {view === "home" && (
        <HomePage
          onOpenProject={openProject}
          onOpenSkills={() => setView("skills")}
          onOpenControlPlane={() => setView("control-plane")}
          onOpenBenchmarks={() => setView("benchmarks")}
          onOpenSettings={() => setView("settings")}
          onOpenProfile={() => setView("profile")}
        />
      )}

      {view === "workspace" && activeProject && (
        <WorkspacePage
          sessionId={activeProject}
          onBackHome={backToHome}
          onOpenSkills={() => setView("skills")}
          onOpenSettings={() => setView("settings")}
          onOpenSession={openProject}
        />
      )}

      {view === "skills"   && <SkillsPage   onBack={backToHome} />}
      {view === "control-plane" && <ControlPlanePage onBack={backToHome} />}
      {view === "benchmarks" && <BenchmarksPage onBack={backToHome} />}
      {view === "profile"  && <ProfilePage  onBack={backToHome} />}
      {view === "settings" && <SettingsPage onBack={() => setView(activeProject ? "workspace" : "home")} />}

      {/* First-run onboarding overlay — gates the app behind a 3-step setup
          on first launch. Dismissable + remembered via settings.onboarded. */}
      {needsOnboarding && (
        <OnboardingOverlay
          onClose={() => setOnboardingDismissed(true)}
          onPickNewProject={onboardingNewProject}
          onPickQuickTask={onboardingQuickTask}
          onPickProfile={onboardingProfile}
        />
      )}

      {/* Global toast notifications for evidence pack events */}
      <ToastContainer onViewPack={(path) => setEvidenceViewerPath(path)} />

      {/* Global evidence viewer overlay */}
      {evidenceViewerPath && (
        <EvidenceViewer
          packPath={evidenceViewerPath}
          onClose={() => setEvidenceViewerPath(null)}
        />
      )}
    </>
  );
}
