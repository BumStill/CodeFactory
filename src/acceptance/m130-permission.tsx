// SPDX-License-Identifier: Apache-2.0
import React from "react";
import { createRoot } from "react-dom/client";
import "../styles/globals.css";
import { PermissionDialog } from "../components/PermissionDialog";
import { PermissionModePicker } from "../components/PermissionModePicker";
import { useChatStore } from "../stores/chat";
useChatStore.setState({ activeSession: { id: "synthetic-m130-session", title: "Synthetic", cwd: "/synthetic", model_id: "synthetic", created_at: 1, updated_at: 1, total_input_tokens: 0, total_output_tokens: 0, kind: "project", permission_mode: "trusted", delivery_authorized: false }, sessions: [] });
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = { invoke: async (command: string, args: { authorized?: boolean }) => {
  if (command === "update_session_delivery_authorized") {
    (window as unknown as Record<string, unknown>).__m130Authorized = args.authorized;
    return null;
  }
  throw new Error(`Unexpected acceptance command: ${command}`);
} };
function App() {
  const [decision, setDecision] = React.useState("waiting_user");
  return <main aria-label="M130 delivery approval" className="min-h-screen bg-bg text-gray-200">
    <output data-testid="decision">{decision}</output>
    <div className="fixed bottom-4 left-4"><PermissionModePicker /></div>
    {decision === "waiting_user" && <PermissionDialog trusted request={{ intentId: "synthetic-m130", toolCallId: "synthetic-push", toolName: "deliver_changes", args: { ceiling: "pr_only", title: "Synthetic delivery" } }} onAllow={() => setDecision("delivery_authorized")} onDeny={() => setDecision("local_only")} onAllowFullAccess={() => { throw new Error("Trust must not authorize delivery"); }} />}
  </main>;
}
createRoot(document.getElementById("root")!).render(<App />);
