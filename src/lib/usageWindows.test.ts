import { describe, it, expect } from "vitest";
import { evaluateWindow, applyPreset, modelWindowKey } from "./usageWindows";

describe("CF-WINDOW-R1/R3 deterministic model windows", () => {
  const preset = () => applyPreset("deepseek_off_peak");
  const at = (clock: string) => Date.parse(`2026-10-13T${clock}:00+08:00`);
  it.each([["08:59",true],["09:00",false],["09:01",false],["11:59",false],["12:00",true],["12:01",true],["13:59",true],["14:00",false],["14:01",false],["17:59",false],["18:00",true],["18:01",true]])("boundary %s", (clock, allowed) => {
    expect(evaluateWindow(preset(), at(clock as string)).available).toBe(allowed);
  });
  it("treats a year with no bundled notice conservatively and states which years have data", () => {
    const rule = preset();
    // 2026 has data: 10-01 is a holiday → off-peak all day.
    expect(evaluateWindow(rule, Date.parse("2026-10-01T10:00:00+08:00")).available).toBe(true);
    // 2026-10-10 is an adjusted working Saturday → peak.
    expect(evaluateWindow(rule, Date.parse("2026-10-10T10:00:00+08:00")).available).toBe(false);
    // 2027 has no published notice yet: weekday 10:00 stays peak (conservative),
    // and no holiday is assumed, never guessed.
    expect(rule.calendar?.years).toEqual([2026]);
    expect(evaluateWindow(rule, Date.parse("2027-10-01T10:00:00+08:00")).available).toBe(false);
    expect(evaluateWindow(rule, Date.parse("2027-10-03T10:00:00+08:00")).available).toBe(true);
  });
  it("returns exact next boundary", () => {
    expect(evaluateWindow(preset(), at("09:00")).next_transition_ms).toBe(at("12:00"));
    expect(evaluateWindow(preset(), at("13:59")).next_transition_ms).toBe(at("14:00"));
  });
  it("weekends and missing calendar years are conservative", () => {
    expect(evaluateWindow(preset(), Date.parse("2031-10-18T10:00:00+08:00")).available).toBe(true);
    expect(evaluateWindow(preset(), Date.parse("2031-10-20T10:00:00+08:00")).available).toBe(false);
  });
  it("honours synthetic holidays and adjusted working weekends", () => {
    const rule = preset();
    rule.calendar = { years: [2026], holidays: ["2026-10-13"], workdays: ["2026-10-17"] };
    expect(evaluateWindow(rule, at("10:00")).available).toBe(true);
    expect(evaluateWindow(rule, Date.parse("2026-10-17T10:00:00+08:00")).available).toBe(false);
  });
  it("same endpoint has independent model rules; same model across endpoints too", () => {
    const rules = { [modelWindowKey("a", "pro")]: preset(), [modelWindowKey("a", "flash")]: { mode: "anytime" as const }, [modelWindowKey("b", "pro")]: { mode: "anytime" as const } };
    expect(evaluateWindow(rules[modelWindowKey("a", "pro")], at("10:00")).available).toBe(false);
    expect(evaluateWindow(rules[modelWindowKey("a", "flash")], at("10:00")).available).toBe(true);
    expect(evaluateWindow(rules[modelWindowKey("b", "pro")], at("10:00")).available).toBe(true);
    expect(JSON.parse(JSON.stringify(rules))).toEqual(rules);
  });
  it("preset fills a distinct editable copy", () => {
    const first = preset(); first.periods = [];
    expect(preset().periods.length).toBe(2);
    expect(evaluateWindow(undefined, at("10:00"))).toEqual({ available: true, next_transition_ms: null });
  });
  it("custom overnight period respects local weekday", () => {
    const rule = { mode: "custom" as const, timezone: "America/New_York", weekdays: [1], periods: [{ start: "22:00", end: "02:00" }] };
    expect(evaluateWindow(rule, Date.parse("2026-10-20T05:00:00Z")).available).toBe(true);
    expect(evaluateWindow(rule, Date.parse("2026-10-20T07:00:00Z")).available).toBe(false);
  });
});
