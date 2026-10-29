// SPDX-License-Identifier: Apache-2.0
import { useChatStore } from "../stores/chat";
import type { Session } from "./tauri";
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
    createAndSend: async ({ clientRequestId, project, message, model, permissionMode, deliveryAuthorized, releaseUrgency }) => {
      const selected = model ? await dispatchSetModel({ model }) : null;
      const store = useChatStore.getState();
      // M200/R3: a keyed dispatch must be durable before it is sent, so a
      // repeated request reuses the same session instead of creating another.
      let session: Session;
      let rootTurnId: string | undefined;
      if (clientRequestId) {
        [session, rootTurnId] = await invoke<[Session, string]>("materialize_dispatch_session", {
          clientRequestId, cwd: project, modelId: selected?.model ?? store.activeModel, firstMessage: message,
        });
        await store.loadSessions();
        await store.selectSession(session.id);
      } else {
        session = await store.createSession(project, selected?.model ?? store.activeModel);
      }
      if (permissionMode) await dispatchSetPermission({ sessionId: session.id, mode: permissionMode });
      // The durable row already IS the first message, so keyed dispatch resumes it.
      const receipt = await dispatchSend({ sessionId: session.id, message, mode: "steer", rootTurnId, deliveryAuthorized, releaseUrgency });
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
    closeObjective: async ({ sessionId, outcome, objectiveId }) => {
      const result = await invoke("close_objective", { sessionId, outcome, objectiveId });
      await useChatStore.getState().loadSessions();
      return result;
    },
    status: dispatchStatus,
  } });
}
