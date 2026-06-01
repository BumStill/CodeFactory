// SPDX-License-Identifier: Apache-2.0

/**
 * Human "x ago" label for an absolute timestamp in **milliseconds**.
 *
 * The backend stamps `created_at` / `updated_at` with
 * `Utc::now().timestamp_millis()`, so values arrive in MILLISECONDS. An
 * earlier copy of this helper multiplied the input by 1000 (treating it as
 * seconds), which pushed every timestamp ~50,000 years into the future and
 * made the diff permanently negative — so every Recent list showed "刚刚".
 * This version consumes milliseconds directly.
 *
 * `now` is injectable so the formatting is deterministic under test.
 */
export function formatRelativeTime(ms: number, now: number = Date.now()): string {
  const diff = now - ms;
  const min = 60 * 1000;
  const hr = 60 * min;
  const day = 24 * hr;
  if (diff < min) return "刚刚";
  if (diff < hr) return `${Math.floor(diff / min)} 分钟前`;
  if (diff < day) return `${Math.floor(diff / hr)} 小时前`;
  if (diff < 7 * day) return `${Math.floor(diff / day)} 天前`;
  return new Date(ms).toLocaleDateString();
}
