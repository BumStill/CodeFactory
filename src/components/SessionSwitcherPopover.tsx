// SPDX-License-Identifier: Apache-2.0
//
// Floating quick-switcher shown when the Workspace session sidebar is
// collapsed. Anchored under the header's sidebar-toggle button, it reuses the
// full SessionSidebar (unified quick+project list + "+ 新建") so collapsing the
// rail never buries session switching — one click on the top-left icon brings
// the whole list back as an overlay. Dismissal (click-outside) is owned by the
// parent (it wraps both the trigger button and this popover in one ref), so
// this component stays purely presentational.
import { PanelLeftOpen } from "lucide-react";
import { SessionSidebar } from "./SessionSidebar";

interface SessionSwitcherPopoverProps {
  currentSessionId: string;
  /** Switch to a session. The parent also closes the popover on select. */
  onOpenSession: (id: string) => void;
  /** Pin the sidebar back open (dock it) and dismiss the popover. */
  onExpand: () => void;
}

export function SessionSwitcherPopover({
  currentSessionId,
  onOpenSession,
  onExpand,
}: SessionSwitcherPopoverProps) {
  return (
    <div className="absolute left-0 top-full z-50 mt-1 flex h-[70vh] max-h-[32rem] w-64 flex-col overflow-hidden rounded-lg border border-border bg-surface-1 shadow-2xl">
      <div className="flex items-center justify-between border-b border-border px-2.5 py-1.5">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">
          快速切换会话
        </span>
        <button
          onClick={onExpand}
          title="把会话列表固定到左侧栏"
          className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium text-accent transition-colors hover:bg-accent/10"
        >
          <PanelLeftOpen size={12} />
          固定到左侧
        </button>
      </div>
      <SessionSidebar currentSessionId={currentSessionId} onOpenSession={onOpenSession} />
    </div>
  );
}
