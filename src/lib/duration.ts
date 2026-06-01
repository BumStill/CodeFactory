// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";

/**
 * Compact, human duration for a single chat turn.
 *
 *   < 1s   → "850ms"
 *   < 1min → "12.7s"   (one decimal)
 *   < 1hr  → "1m07s"
 *   ≥ 1hr  → "1h02m"
 *
 * Clamps negatives / non-finite input to 0 so clock skew or a missing start
 * timestamp can never render garbage.
 */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) ms = 0;
  if (ms < 1000) return `${Math.round(ms)}ms`;

  const totalSec = ms / 1000;
  if (totalSec < 60) return `${totalSec.toFixed(1)}s`;

  const totalMin = Math.floor(totalSec / 60);
  if (totalMin < 60) {
    const sec = Math.floor(totalSec % 60);
    return `${totalMin}m${String(sec).padStart(2, "0")}s`;
  }

  const hr = Math.floor(totalMin / 60);
  const min = totalMin % 60;
  return `${hr}h${String(min).padStart(2, "0")}m`;
}

/**
 * Re-render on a ~1s cadence while `active`, so an elapsed-time label keeps
 * ticking even when nothing else changes — e.g. during a long tool call that
 * emits no text deltas. Returns the current `Date.now()` sample.
 *
 * When `active` is false the timer is torn down and the last sample is held,
 * so a settled turn doesn't keep waking React.
 */
export function useNowTick(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now()); // fresh sample the moment we activate
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}
