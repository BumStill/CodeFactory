// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import {
  describeUpdateBlockers,
  useUpdaterStore,
  type UpdateSafetyStatus,
} from "../stores/updater";

/** Lists what an update is waiting for and lets the user wait or update now. */
export function UpdateBlockerChoice({ blockers }: { blockers: UpdateSafetyStatus }) {
  const forceRequested = useUpdaterStore((s) => s.forceRequested);
  const forceInstall = useUpdaterStore((s) => s.forceInstall);
  const minimized = useUpdaterStore((s) => s.updateMinimized);
  const setMinimized = useUpdaterStore((s) => s.setUpdateMinimized);
  const [confirming, setConfirming] = useState(false);

  const button = "flex-1 rounded border border-border py-1 text-caption transition-colors hover:bg-surface-3";
  return (
    <div className="space-y-1.5 pt-1">
      <ul className="list-disc pl-4 text-caption text-gray-500">
        {describeUpdateBlockers(blockers).map((line) => <li key={line}>{line}</li>)}
      </ul>
      {forceRequested ? (
        <p className="text-caption text-gray-500">正在立即更新…</p>
      ) : confirming ? (
        <>
          <p className="text-caption text-gray-500">上面的工作会被中断，应用随后重启。</p>
          <div className="flex gap-2">
            <button type="button" className={`${button} text-gray-500`} onClick={() => setConfirming(false)}>
              取消
            </button>
            <button type="button" className={`${button} text-accent font-medium`} onClick={() => void forceInstall()}>
              中断并更新
            </button>
          </div>
        </>
      ) : (
        <>
          {minimized && <p className="text-caption text-gray-500">完成后会自动更新。</p>}
          <div className="flex gap-2">
            {!minimized && (
              <button type="button" className={`${button} text-gray-500`} onClick={() => setMinimized(true)}>
                等完成后更新
              </button>
            )}
            <button type="button" className={`${button} text-accent font-medium`} onClick={() => setConfirming(true)}>
              立即更新
            </button>
          </div>
        </>
      )}
    </div>
  );
}
