// SPDX-License-Identifier: Apache-2.0
export declare function captureScreenshot(
  page: { screenshot: (options: Record<string, unknown>) => Promise<unknown> },
  file: string,
  options?: {
    log?: (line: string) => void;
    fullPageTimeoutMs?: number;
    viewportTimeoutMs?: number;
  },
): Promise<{ fullPage: boolean; error?: unknown }>;
