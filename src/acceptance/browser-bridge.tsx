import { createRoot } from "react-dom/client";
import { BrowserBridgePanel } from "../pages/Settings/SettingsPage";
createRoot(document.getElementById("root")!).render(<main className="min-h-screen bg-surface-0 p-4"><BrowserBridgePanel /></main>);
