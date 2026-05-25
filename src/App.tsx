// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { HomePage } from "./pages/Home/HomePage";
import { WorkspacePage } from "./pages/Workspace/WorkspacePage";
import { SpecsPage } from "./pages/Specs/SpecsPage";
import { SkillsPage } from "./pages/Skills/SkillsPage";
import { SettingsPage } from "./pages/Settings/SettingsPage";
import { ToastContainer } from "./components/Toast";
import { EvidenceViewer } from "./components/EvidenceViewer";
import { UpdaterBanner } from "./components/UpdaterBanner";
import { useSettingsStore } from "./stores/settings";

export type AppView = "home" | "workspace" | "specs" | "skills" | "settings";

export default function App() {
  const [view, setView] = useState<AppView>("home");
  const [activeProject, setActiveProject] = useState<string | null>(null);
  const [evidenceViewerPath, setEvidenceViewerPath] = useState<string | null>(null);
  const loadSettings = useSettingsStore((s) => s.load);

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

  return (
    <>
      <UpdaterBanner />

      {view === "home" && (
        <HomePage
          onOpenProject={openProject}
          onOpenSpecs={() => setView("specs")}
          onOpenSkills={() => setView("skills")}
          onOpenSettings={() => setView("settings")}
        />
      )}

      {view === "workspace" && activeProject && (
        <WorkspacePage
          sessionId={activeProject}
          onBackHome={backToHome}
          onOpenSettings={() => setView("settings")}
        />
      )}

      {view === "specs"    && <SpecsPage    onBack={backToHome} />}
      {view === "skills"   && <SkillsPage   onBack={backToHome} />}
      {view === "settings" && <SettingsPage onBack={() => setView(activeProject ? "workspace" : "home")} />}

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
