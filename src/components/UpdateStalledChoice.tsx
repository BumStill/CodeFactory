// SPDX-License-Identifier: Apache-2.0
import { invoke } from "../lib/tauri";
import { useUpdaterStore } from "../stores/updater";

const RELEASES_URL = "https://github.com/BumStill/CodeFactory/releases/tag";

async function openReleasePage(version: string) {
  const url = `${RELEASES_URL}/v${version}`;
  try {
    await invoke("plugin:shell|open", { path: url });
  } catch {
    window.open(url, "_blank");
  }
}

/** The way out when an update has nothing to wait for and still is not moving. */
export function UpdateStalledChoice({ version, detail }: { version: string; detail: string | null }) {
  const retryInstall = useUpdaterStore((s) => s.retryInstall);

  const button = "flex-1 rounded border border-border py-1 text-caption transition-colors hover:bg-surface-3";
  return (
    <div className="space-y-1.5 pt-1">
      <p className="text-caption text-gray-500">
        没有任务在占用，但更新一直没开始。可能是网络不通，或上次下载、安装失败了。
      </p>
      {detail && <p className="text-caption text-gray-500 break-words">{detail}</p>}
      <div className="flex gap-2">
        <button type="button" className={`${button} text-gray-500`} onClick={() => void openReleasePage(version)}>
          手动下载
        </button>
        <button type="button" className={`${button} text-accent font-medium`} onClick={() => void retryInstall()}>
          重试
        </button>
      </div>
    </div>
  );
}
