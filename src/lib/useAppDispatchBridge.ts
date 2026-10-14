// SPDX-License-Identifier: Apache-2.0
import { useChatStore } from "../stores/chat";
import { invoke } from "./tauri";
import { useDesktopDispatchBridge } from "./useDesktopDispatchBridge";
import { getUsageWindows, setUsageWindow, setSessionUsageWindowOverride } from "./modelUsageWindows";
import { getQuota, setQuotaCap } from "./subscriptionQuota";
import { dispatchSend, dispatchSetModel, dispatchSetPermission, dispatchStatus, dispatchStop } from "./dispatchActions";
import type { DispatchApproval } from "./desktopDispatch";

/** M83: app-owned receiver; no composer ref or page lifetime dependency. */
export function useAppDispatchBridge(showWorkspace: () => void): void {
  useDesktopDispatchBridge({ handlers: {
    getUsageWindows, setUsageWindow, setSessionUsageWindowOverride, getQuota, setQuotaCap,
    createAndSend: async ({ project, message, model, permissionMode }) => {
      const selected = model ? await dispatchSetModel({ model }) : null;
      const store = useChatStore.getState();
      const session = await store.createSession(project, selected?.model ?? store.activeModel);
      if (permissionMode) await dispatchSetPermission({ sessionId: session.id, mode: permissionMode });
      const receipt = await dispatchSend({ sessionId: session.id, message, mode: "steer" });
      return { ...receipt, model: session.model_id };
    },
    send: dispatchSend,
    setPermission: dispatchSetPermission,
    setModel: dispatchSetModel,
    switchSession: async ({ sessionId }) => {
      await useChatStore.getState().selectSession(sessionId);
      showWorkspace();
      return { session_id: sessionId };
    },
    stop: dispatchStop,
    cleanBuildCache: () => invoke("build_cache_cleanup"),
    listApprovals: async () => invoke<DispatchApproval[]>("list_pending_approvals"),
    resolveApproval: async ({ approvalId, approve }) => {
      await invoke("respond_to_permission", { intentId: approvalId, allow: approve });
      return { approval_id: approvalId, approved: approve };
    },
    closeObjective: async ({ sessionId, outcome }) => {
      const result = await invoke("close_objective", { sessionId, outcome });
      await useChatStore.getState().loadSessions();
      return result;
    },
    status: dispatchStatus,
  } });
}
