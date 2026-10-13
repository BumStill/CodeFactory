// SPDX-License-Identifier: Apache-2.0
import presets from "./usageWindowPresets.json";

export interface UsageWindow {
  mode: "anytime" | "official_off_peak" | "custom";
  timezone?: string;
  weekdays?: number[];
  periods?: { start: string; end: string }[];
  exclude_holidays?: boolean;
  invert?: boolean;
  calendar?: { years: number[]; holidays: string[]; workdays: string[] };
}
export interface WindowStatus { available: boolean; next_transition_ms: number | null }
/** Tuple encoding prevents endpoint/model delimiter collisions. */
export const modelWindowKey = (endpoint: string, model: string): string => JSON.stringify([endpoint, model]);
export function applyPreset(id: keyof typeof presets): UsageWindow & { periods: {start: string; end: string}[] } {
  return structuredClone(presets[id]) as UsageWindow & { periods: {start: string; end: string}[] };
}
function localParts(ms: number, timezone: string) {
  const parts = new Intl.DateTimeFormat("en-CA", { timeZone: timezone, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).formatToParts(ms);
  const get = (key: string) => parts.find(p => p.type === key)!.value;
  const date = `${get("year")}-${get("month")}-${get("day")}`;
  return { date, weekday: new Date(`${date}T12:00:00Z`).getUTCDay(), minute: Number(get("hour")) * 60 + Number(get("minute")) };
}
const minute = (clock: string) => Number(clock.slice(0, 2)) * 60 + Number(clock.slice(3));
function available(rule: UsageWindow, ms: number, formatter: (ms: number) => ReturnType<typeof localParts>): boolean {
  const local = formatter(ms);
  const calendar = rule.calendar;
  // CF-WINDOW-R3: a year the bundled notice does not cover has NO holiday
  // data. Never guess: treat holidays/workdays as unknown and fall back to the
  // conservative rule (a weekday is peak). The Settings section says so.
  const covered = (calendar?.years ?? []).includes(Number(local.date.slice(0, 4)));
  const working = (date: string, weekday: number) => {
    if (covered && calendar?.workdays.includes(date)) return true;
    if (rule.exclude_holidays && covered && calendar?.holidays.includes(date)) return false;
    return (rule.weekdays ?? []).includes(weekday);
  };
  const previousDate = new Date(`${local.date}T12:00:00Z`);
  previousDate.setUTCDate(previousDate.getUTCDate() - 1);
  const previous = previousDate.toISOString().slice(0, 10);
  const matches = (rule.periods ?? []).some(period => {
    const start = minute(period.start), end = minute(period.end);
    if (start < end) return working(local.date, local.weekday) && local.minute >= start && local.minute < end;
    return (working(local.date, local.weekday) && local.minute >= start) || (working(previous, previousDate.getUTCDay()) && local.minute < end);
  });
  return rule.invert ? !matches : matches;
}
/** Injected epoch clock; independent of the operating system's timezone. */
export function evaluateWindow(rule: UsageWindow | undefined, nowMs: number): WindowStatus {
  if (!rule || rule.mode === "anytime") return { available: true, next_transition_ms: null };
  const formatter = (ms: number) => localParts(ms, rule.timezone ?? "UTC");
  const current = available(rule, nowMs, formatter);
  // Minute-resolution rules: inspect the next minute boundary, not now+60s.
  const first = Math.floor(nowMs / 60000) * 60000 + 60000;
  for (let ms = first; ms <= first + 8 * 86400000; ms += 60000) {
    if (available(rule, ms, formatter) !== current) return { available: current, next_transition_ms: ms };
  }
  return { available: current, next_transition_ms: null };
}
