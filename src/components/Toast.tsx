// SPDX-License-Identifier: Apache-2.0
//! Toast notification system for evidence pack ready events.

import { X, Package } from "lucide-react";
import { useToastStore, type EvidenceNotification } from "../stores/tasks";

interface ToastProps {
  notification: EvidenceNotification;
  onView: (path: string) => void;
}

function ToastItem({ notification, onView }: ToastProps) {
  const { dismissNotification } = useToastStore();

  return (
    <div className="flex items-start gap-3 w-80 rounded-lg border border-border bg-surface-2 shadow-xl p-3 animate-in slide-in-from-bottom-2">
      <div className="p-1.5 rounded bg-green-900/40 shrink-0">
        <Package size={14} className="text-green-400" />
      </div>
      <div className="flex-1 min-w-0">
        <div className="text-label font-semibold text-gray-200">
          Evidence pack ready
        </div>
        <div className="text-caption text-gray-400 truncate mt-0.5">
          {notification.spec_req_id} — {notification.spec_title}
        </div>
        <button
          onClick={() => {
            onView(notification.path);
            dismissNotification(notification.id);
          }}
          className="mt-1.5 text-caption text-accent hover:text-accent-hover transition-colors"
        >
          View Evidence Pack →
        </button>
      </div>
      <button
        onClick={() => dismissNotification(notification.id)}
        className="p-0.5 rounded text-gray-600 hover:text-gray-300 transition-colors shrink-0"
      >
        <X size={12} />
      </button>
    </div>
  );
}

interface ToastContainerProps {
  onViewPack: (path: string) => void;
}

export function ToastContainer({ onViewPack }: ToastContainerProps) {
  const { notifications } = useToastStore();

  if (notifications.length === 0) return null;

  return (
    <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2">
      {notifications.map((n) => (
        <ToastItem key={n.id} notification={n} onView={onViewPack} />
      ))}
    </div>
  );
}
