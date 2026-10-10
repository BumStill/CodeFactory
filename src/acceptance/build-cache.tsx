// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the build-cache panel (CF-BLD-R4). It
// mounts the production component with an injected data source so a real engine
// verifies the occupancy numbers, the one-click cleanup and the light/dark
// rendering — jsdom cannot resolve the theme tokens, and a stubbed command name
// would not prove the panel shows what the runtime actually reports.

import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import BuildCachePanel, {
  type BuildCacheReport,
  type BuildCacheSource,
  type MaintenanceOutcome,
} from "../components/BuildCachePanel";

const GIB = 1024 ** 3;

function report(totalBytes: number, waiting: boolean): BuildCacheReport {
  return {
    total_bytes: totalBytes,
    budget_bytes: 60 * GIB,
    entries: [
      {
        path: "/Users/you/Library/Application Support/com.codefactory.app/execution-workspaces/a1/.codefactory-cache/cargo-target-test",
        bytes: 5 * GIB,
        files: 4_210,
        last_used_unix: 1_760_000_000,
        in_use: false,
        owner: "a1",
      },
      {
        path: "/Users/you/Library/Application Support/com.codefactory.app/execution-workspaces/b2/.codefactory-cache/cargo-target-test",
        bytes: 7 * GIB,
        files: 6_180,
        last_used_unix: 1_760_000_600,
        in_use: true,
        owner: "b2",
      },
    ],
    heavy_builds_running: waiting ? 2 : 0,
    heavy_builds_waiting: waiting ? 2 : 0,
    heavy_build_limit: 2,
    heavy_build_status: waiting ? "等待编译空位（前面还有 2 个构建）" : null,
  };
}

let totalBytes = 12 * GIB;

const source: BuildCacheSource = {
  report: async () => report(totalBytes, true),
  cleanup: async (): Promise<MaintenanceOutcome> => {
    const before = totalBytes;
    totalBytes = 5 * GIB;
    return {
      scanned: 2,
      before_bytes: before,
      after_bytes: totalBytes,
      reclaimed_bytes: before - totalBytes,
      evicted_bytes: 0,
      protected_bytes: totalBytes,
      overflow_bytes: 0,
      removed: ["a1/.codefactory-cache/cargo-target-test"],
    };
  },
};

/** Every state the acceptance assertions read from, in one page. */
function Page() {
  return (
    <main aria-label="Build cache acceptance" className="bg-surface-0 p-6">
      <div data-fixture="occupancy" className="w-[820px]">
        <BuildCachePanel source={source} />
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);
