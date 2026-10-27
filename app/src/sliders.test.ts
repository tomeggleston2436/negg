// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import settingsFixture from "./ipc/__fixtures__/settings.json";
import { type FrameOverrides, type Settings, decodeSettings, emptyOverrides } from "./ipc/types";
import {
  SLIDERS,
  clampToSpec,
  effectiveGeometry,
  enteredValue,
  formatValue,
  outOfRange,
  parseValue,
  sliderPosition,
  stepDecimals,
  turn,
  withGeometry,
} from "./sliders";

const settings: Settings = decodeSettings(settingsFixture);

function spec(id: string) {
  const s = SLIDERS.find((x) => x.id === id);
  if (!s) throw new Error(id);
  return s;
}

describe("saturation slider", () => {
  it("reads an absent saturation as 1, the override first", () => {
    const o = emptyOverrides();
    expect(settings.print.saturation).toBeUndefined();
    expect(spec("saturation").get(o, settings)).toBe(1);
    o.print.saturation = 0.4;
    expect(spec("saturation").get(o, settings)).toBe(0.4);
    const s = structuredClone(settings);
    s.print.saturation = 1.5;
    expect(spec("saturation").get(emptyOverrides(), s)).toBe(1.5);
  });

  it("sets a whole value without touching the input or the preset label", () => {
    const o = emptyOverrides();
    const a = spec("saturation").set(o, 1.25, settings);
    expect(a.print.saturation).toBe(1.25);
    expect(o.print.saturation ?? null).toBeNull();
    expect(a.print.preset ?? null).toBeNull();
  });
});

describe("slider table", () => {
  it("lists the table in order with its ranges and defaults", () => {
    expect(SLIDERS.map((s) => [s.id, s.min, s.max, s.step, s.defaultValue])).toEqual([
      ["density", -1, 1, 0.01, 0],
      ["grade", 50, 180, 1, 115],
      ["cyan", -0.3, 0.3, 0.005, 0],
      ["magenta", -0.3, 0.3, 0.005, 0],
      ["yellow", -0.3, 0.3, 0.005, 0],
      ["toe", 1, 20, 0.1, 6],
      ["shoulder", 0.5, 10, 0.1, 3],
      ["saturation", 0, 2, 0.01, 1],
      ["analysis_buffer", 0, 0.2, 0.01, 0.05],
    ]);
  });

  it("defaults match the model defaults in the settings fixture", () => {
    const o = emptyOverrides();
    for (const s of SLIDERS) expect(s.get(o, settings)).toBe(s.defaultValue);
  });

  it("reads the override first, then the resolved setting", () => {
    const o = emptyOverrides();
    o.print.exposure = 0.4;
    expect(spec("density").get(o, settings)).toBe(0.4);
    expect(spec("grade").get(o, settings)).toBe(115);
  });

  it("sets whole values into the right fields without touching the input", () => {
    const o = emptyOverrides();
    const a = spec("density").set(o, -0.2, settings);
    expect(a.print.exposure).toBe(-0.2);
    expect(o.print.exposure).toBeNull();
    expect(spec("grade").set(o, 90).print.grade).toBe(90);
    expect(spec("toe").set(o, 7.5).print.toe).toBe(7.5);
    expect(spec("shoulder").set(o, 2.5).print.shoulder).toBe(2.5);
  });

  it("sets one CMY component from the resolved triple", () => {
    const s: Settings = structuredClone(settings);
    s.print.cmy = [0.01, 0.02, 0.03];
    const o = spec("magenta").set(emptyOverrides(), -0.1, s);
    expect(o.print.cmy).toEqual([0.01, -0.1, 0.03]);
    const o2 = spec("yellow").set(o, 0.2, s);
    expect(o2.print.cmy).toEqual([0.01, -0.1, 0.2]);
    expect(spec("cyan").get(o2, s)).toBe(0.01);
  });

  it("sets the analysis buffer as a whole geometry override", () => {
    const s: Settings = structuredClone(settings);
    s.geometry = { crop: { x: 0.1, y: 0.1, width: 0.8, height: 0.8 }, analysis_buffer: 0.05, rotate_degrees: 90, flip_horizontal: true };
    const o = spec("analysis_buffer").set(emptyOverrides(), 0.12, s);
    expect(o.geometry).toEqual({ ...s.geometry, analysis_buffer: 0.12 });
    expect(spec("analysis_buffer").get(o, settings)).toBe(0.12);
  });
});

describe("clamping and entry", () => {
  it("snaps to the step and clamps to the range", () => {
    expect(clampToSpec(spec("grade"), 200)).toBe(180);
    expect(clampToSpec(spec("grade"), 49.4)).toBe(50);
    expect(clampToSpec(spec("grade"), 99.6)).toBe(100);
    expect(clampToSpec(spec("cyan"), 0.1234)).toBe(0.125);
    expect(clampToSpec(spec("cyan"), -1)).toBe(-0.3);
    expect(clampToSpec(spec("density"), 0.1 + 0.2)).toBe(0.3);
    expect(clampToSpec(spec("toe"), 6.04)).toBe(6);
    expect(clampToSpec(spec("density"), Number.NaN)).toBe(-1);
  });

  it("shows an out-of-range sidecar value clamped on the track only", () => {
    const g = spec("grade");
    expect(outOfRange(g, 200)).toBe(true);
    expect(sliderPosition(g, 200)).toBe(180);
    expect(outOfRange(g, 120)).toBe(false);
    expect(formatValue(g, 200)).toBe("200");
  });

  it("formats with the step's decimals and parses typed text", () => {
    expect(stepDecimals(0.005)).toBe(3);
    expect(stepDecimals(1)).toBe(0);
    expect(formatValue(spec("cyan"), -0.1)).toBe("-0.100");
    expect(parseValue(" 0.25 ")).toBe(0.25);
    expect(parseValue("−0.5")).toBe(-0.5);
    expect(parseValue("")).toBeNull();
    expect(parseValue("abc")).toBeNull();
  });
});

describe("geometry helpers", () => {
  it("turns in quarter steps both ways", () => {
    expect(turn(0, 1)).toBe(90);
    expect(turn(270, 1)).toBe(0);
    expect(turn(0, -1)).toBe(270);
    expect(turn(180, -1)).toBe(90);
  });

  it("patches the effective geometry", () => {
    const o: FrameOverrides = emptyOverrides();
    expect(effectiveGeometry(o, settings)).toEqual(settings.geometry);
    const flipped = withGeometry(o, { flip_horizontal: true }, settings);
    expect(flipped.geometry).toEqual({ ...settings.geometry, flip_horizontal: true });
    const cleared = withGeometry(flipped, { crop: null });
    expect(cleared.geometry?.flip_horizontal).toBe(true);
  });
});

describe("typed entry (Ruling 78: never rewrite until the user changes it)", () => {
  it("does not commit when the text is unchanged since focus", () => {
    const g = spec("grade");
    // Tabbing through an out-of-range sidecar value.
    expect(enteredValue(g, "200", formatValue(g, 200), 200)).toBeNull();
    // An off-step value shown rounded (exposure 0.123 shows 0.12).
    const d = spec("density");
    expect(enteredValue(d, "0.12", formatValue(d, 0.123), 0.123)).toBeNull();
    expect(enteredValue(d, " 0.12 ", "0.12", 0.123)).toBeNull();
  });

  it("commits a changed entry snapped and clamped", () => {
    const g = spec("grade");
    expect(enteredValue(g, "250", "200", 200)).toBe(180);
    expect(enteredValue(g, "99.6", "115", 115)).toBe(100);
    const d = spec("density");
    expect(enteredValue(d, "0.13", "0.12", 0.123)).toBe(0.13);
  });

  it("ignores text that is not a number or that lands on the current value", () => {
    const g = spec("grade");
    expect(enteredValue(g, "abc", "115", 115)).toBeNull();
    expect(enteredValue(g, "115.2", "115", 115)).toBeNull();
  });
});
