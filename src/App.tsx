// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import { ChatPage } from "./pages/Chat/ChatPage";
import { SpecsPage } from "./pages/Specs/SpecsPage";
import { SkillsPage } from "./pages/Skills/SkillsPage";
import { ToastContainer } from "./components/Toast";
import { EvidenceViewer } from "./components/EvidenceViewer";

export type AppView = "chat" | "specs" | "skills";

export default function App() {
  const [view, setView] = useState<AppView>("chat");
  const [evidenceViewerPath, setEvidenceViewerPath] = useState<string | null>(null);

  return (
    <>
      {view === "specs" && <SpecsPage onBack={() => setView("chat")} />}
      {view === "skills" && <SkillsPage onBack={() => setView("chat")} />}
      {view === "chat" && (
        <ChatPage
          onOpenSpecs={() => setView("specs")}
          onOpenSkills={() => setView("skills")}
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
