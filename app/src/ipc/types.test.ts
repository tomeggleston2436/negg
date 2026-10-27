// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import exportRequest from "./__fixtures__/export_request.json";
import frameOverrides from "./__fixtures__/frame_overrides.json";
import frameView from "./__fixtures__/frame_view.json";
import presetList from "./__fixtures__/preset_list.json";
import negativeView from "./__fixtures__/negative_view.json";
import rollView from "./__fixtures__/roll_view.json";
import settings from "./__fixtures__/settings.json";
import { WireError } from "./tile";
import {
  decodeExportRequest,
  decodeFrameOverrides,
  decodeFrameView,
  decodeNegativeView,
  decodePresetList,
  decodeRollView,
  decodeSettings,
  emptyOverrides,
  presetLabel,
  resolve,
} from "./types";

describe("fixtures decode", () => {
  it("decodes every fixture", () => {
    const s = decodeSettings(settings);
    expect(s.schema_version).toBe(5);
    expect(s.geometry.crop).toBeNull();
    // No analysis area: the key is omitted on the wire.
    expect("analysis_area" in s.geometry).toBe(false);
    const o = decodeFrameOverrides(frameOverrides);
    expect(o.geometry?.rotate_degrees).toBe(90);
    expect(o.geometry?.analysis_area).toEqual({ x: 0.02, y: 0.03, width: 0.96, height: 0.94 });
    const r = decodeRollView(rollView);
    expect(r.frames.map((f) => f.status)).toEqual(["ready", "failed", "read_only", "pending"]);
    expect(r.frames.map((f) => f.needs_analysis_area)).toEqual([true, false, false, false]);
    expect(r.awaiting_analysis).toBe(true);
    const f = decodeFrameView(frameView);
    expect(f.estimates?.cast.cells_used).toBe(123);
    expect(f.pool?.cast_role).toBe("unused");
    expect(f.needs_analysis_area).toBe(true);
    expect(f.settings.geometry.analysis_area).toEqual(o.geometry?.analysis_area);
    const n = decodeNegativeView(negativeView);
    expect([n.index, n.width, n.height, n.levels, n.tile_size]).toEqual([2, 6000, 4000, 5, 512]);
    const e = decodeExportRequest(exportRequest);
    expect(e.scope).toEqual({ kind: "frames", frames: [0, 2] });
  });

  it("the frame view's settings resolve from the roll and its overrides", () => {
    const r = decodeRollView(rollView);
    const f = decodeFrameView(frameView);
    expect(resolve(r.roll.shared, f.overrides)).toEqual(f.settings);
  });

  it("accepts overrides without a geometry key and extra keys", () => {
    const o: Record<string, unknown> = { ...emptyOverrides(), future_field: 1 };
    expect(() => decodeFrameOverrides(o)).not.toThrow();
  });

  it("reads views from before the analysis area: no flags, no area, no awaiting key", () => {
    const old = structuredClone(rollView) as Record<string, unknown> & { frames: Record<string, unknown>[] };
    delete old.awaiting_analysis;
    for (const f of old.frames) delete f.needs_analysis_area;
    const r = decodeRollView(old);
    expect(r.awaiting_analysis).toBeUndefined();
    expect(r.frames[0]?.needs_analysis_area).toBeUndefined();
    const badArea = structuredClone(frameOverrides) as Record<string, unknown> & { geometry: Record<string, unknown> };
    badArea.geometry.analysis_area = { x: 0, y: 0, width: "1", height: 1 };
    expect(() => decodeFrameOverrides(badArea)).toThrow(/FrameOverrides\.geometry\.analysis_area\.width/);
    expect(() => decodeNegativeView({ ...negativeView, levels: -1 })).toThrow(WireError);
  });

  it("rejects drift with the path of the bad field", () => {
    const bad = structuredClone(frameView) as Record<string, unknown> & { settings: { print: Record<string, unknown> } };
    bad.settings.print.grade = "115";
    expect(() => decodeFrameView(bad)).toThrow(/FrameView\.settings\.print\.grade/);
    const badStatus = structuredClone(rollView);
    (badStatus.frames[0] as Record<string, unknown>).status = "done";
    expect(() => decodeRollView(badStatus)).toThrow(WireError);
    const badRotate = structuredClone(frameOverrides) as Record<string, unknown> & { geometry: Record<string, unknown> };
    badRotate.geometry.rotate_degrees = 45;
    expect(() => decodeFrameOverrides(badRotate)).toThrow(WireError);
    expect(() => decodeExportRequest({ scope: { kind: "all" }, format: "jpeg", out_dir: null })).toThrow(WireError);
  });
});

describe("resolve", () => {
  it("copies every Some field and leaves the rest", () => {
    const s = decodeSettings(settings);
    expect(resolve(s, emptyOverrides())).toEqual(s);
    const o = emptyOverrides();
    o.print.toe = 9;
    o.colour.output_space = "rec2020";
    o.film.gamma = { mode: "fixed", value: [1, 2, 3] };
    const r = resolve(s, o);
    expect(r.print.toe).toBe(9);
    expect(r.colour.output_space).toBe("rec2020");
    expect(r.film.gamma).toEqual({ mode: "fixed", value: [1, 2, 3] });
    expect(r.print.grade).toBe(115);
    expect(s.print.toe).toBe(6);
  });
});

describe("saturation and presets", () => {
  it("reads settings with and without saturation and a preset label", () => {
    const plain = decodeSettings(settings);
    expect(plain.print.saturation).toBeUndefined();
    const withSat = structuredClone(settings) as Record<string, unknown> & { print: Record<string, unknown> };
    withSat.print.saturation = 1.3;
    withSat.print.preset = "Rich";
    const s = decodeSettings(withSat);
    expect(s.print.saturation).toBe(1.3);
    expect(presetLabel(s.print)).toBe("Rich");
    withSat.print.saturation = "1.3";
    expect(() => decodeSettings(withSat)).toThrow(/Settings\.print\.saturation/);
  });

  it("treats an absent, null or empty resolved label as no preset", () => {
    expect(presetLabel({})).toBeNull();
    expect(presetLabel({ preset: null })).toBeNull();
    expect(presetLabel({ preset: "" })).toBeNull();
    expect(presetLabel({ preset: "Soft" })).toBe("Soft");
    const withEmpty = structuredClone(settings) as Record<string, unknown> & { print: Record<string, unknown> };
    withEmpty.print.preset = "";
    expect(presetLabel(decodeSettings(withEmpty).print)).toBeNull();
  });

  it("accepts saturation and a pinned empty label in overrides", () => {
    const o = structuredClone(frameOverrides) as Record<string, unknown> & { print: Record<string, unknown> };
    o.print.saturation = 0.8;
    o.print.preset = "";
    const d = decodeFrameOverrides(o);
    expect(d.print.saturation).toBe(0.8);
    expect(d.print.preset).toBe("");
    o.print.preset = 3;
    expect(() => decodeFrameOverrides(o)).toThrow(/FrameOverrides\.print\.preset/);
  });

  it("resolve copies saturation and the label, and a pinned empty label clears it", () => {
    const base = decodeSettings(settings);
    base.print.preset = "Rich";
    const o = emptyOverrides();
    o.print.saturation = 1.4;
    o.print.preset = "Soft";
    const r = resolve(base, o);
    expect(r.print.saturation).toBe(1.4);
    expect(r.print.preset).toBe("Soft");
    o.print.preset = "";
    expect(presetLabel(resolve(base, o).print)).toBeNull();
    expect(base.print.preset).toBe("Rich");
    expect(resolve(base, emptyOverrides()).print.preset).toBe("Rich");
  });

  it("decodes a preset list and rejects drift", () => {
    const list = {
      presets: [{ name: "Rich", look: { grade: 125, toe: 7, shoulder: 3, black_point_compensation: false, saturation: 1.2 } }],
      builtins: 1,
      warnings: ["bad.preset.negg.toml: unreadable"],
    };
    expect(decodePresetList(presetList).presets.map((p) => p.name)).toEqual(["Standard", "Soft"]);
    expect(decodePresetList(presetList).builtins).toBe(2);
    expect(decodePresetList(list).presets[0]?.look.saturation).toBe(1.2);
    expect(() => decodePresetList({ presets: [{ name: "x", look: {} }], builtins: 0, warnings: [] })).toThrow(WireError);
    expect(() => decodePresetList({ presets: [], warnings: [] })).toThrow(WireError);
  });

  it("export requests carry web and tolerate its absence", () => {
    expect(decodeExportRequest(exportRequest).web).toBe("none");
    const old = { ...exportRequest } as Record<string, unknown>;
    delete old.web;
    expect(decodeExportRequest(old).web).toBeUndefined();
    expect(() => decodeExportRequest({ ...exportRequest, web: "both" })).toThrow(WireError);
  });
});
