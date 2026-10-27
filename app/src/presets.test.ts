// SPDX-License-Identifier: Apache-2.0
import { createElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import frameViewFixture from "./ipc/__fixtures__/frame_view.json";
import settingsFixture from "./ipc/__fixtures__/settings.json";
import { type Preset, type Settings, decodeFrameView, decodeSettings } from "./ipc/types";
import {
  PresetPreviewCache,
  ROLL_PRESET_CHOICES,
  chooseRollPreset,
  lookOf,
  nextPreset,
  presetCycleStep,
  presetHeading,
  presetState,
  previewVersion,
  rollPresetQuestion,
  saveNameCheck,
  webOutput,
} from "./presets";
import { PresetStrip, type PresetStripProps } from "./components/PresetStrip";
import { RollPresetPrompt } from "./components/RollPresetPrompt";

const base: Settings = decodeSettings(settingsFixture);

const PRESETS: Preset[] = [
  { name: "Standard", look: { grade: 115, toe: 6, shoulder: 3, black_point_compensation: false, saturation: 1 } },
  { name: "Soft", look: { grade: 95, toe: 4, shoulder: 2, black_point_compensation: false, saturation: 0.95 } },
  { name: "Rich", look: { grade: 130, toe: 8, shoulder: 3.5, black_point_compensation: true, saturation: 1.1 } },
];

function withLook(s: Settings, name: string | null | undefined, look: Preset["look"]): Settings {
  const out = structuredClone(s);
  out.print = { ...out.print, ...look };
  if (name === undefined) delete out.print.preset;
  else out.print.preset = name;
  return out;
}

/** Every element in a tree returned by calling a hookless component. */
function elements(node: ReactNode): ReactElement<Record<string, unknown>>[] {
  if (node === null || node === undefined || typeof node === "boolean") return [];
  if (Array.isArray(node)) return node.flatMap(elements);
  if (typeof node !== "object" || !("props" in node)) return [];
  const el = node as ReactElement<Record<string, unknown>>;
  return [el, ...elements(el.props.children as ReactNode)];
}

function text(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(text).join("");
  if (typeof node === "object" && "props" in node) {
    return text((node as ReactElement<{ children?: ReactNode }>).props.children);
  }
  return "";
}

function buttons(node: ReactNode) {
  return elements(node).filter((e) => e.type === "button");
}

describe("presetState", () => {
  it("names the preset and is unmodified when the look matches it", () => {
    expect(presetState(withLook(base, "Rich", PRESETS[2]!.look), PRESETS)).toEqual({ name: "Rich", modified: false });
  });

  it("is modified when any look value differs", () => {
    const rich = PRESETS[2]!.look;
    for (const patch of [
      { grade: 131 },
      { toe: 7 },
      { shoulder: 3 },
      { black_point_compensation: false },
      { saturation: 1 },
    ]) {
      expect(presetState(withLook(base, "Rich", { ...rich, ...patch }), PRESETS)).toEqual({
        name: "Rich",
        modified: true,
      });
    }
  });

  it("ignores fields a preset never sets", () => {
    const s = withLook(base, "Rich", PRESETS[2]!.look);
    s.print.exposure = 0.4;
    s.print.cmy = [0.1, 0, 0];
    expect(presetState(s, PRESETS)).toEqual({ name: "Rich", modified: false });
  });

  it("reads an absent saturation as 1", () => {
    const s = withLook(base, "Standard", PRESETS[0]!.look);
    delete s.print.saturation;
    expect(presetState(s, PRESETS)).toEqual({ name: "Standard", modified: false });
  });

  it("gives an unknown label with modified true", () => {
    expect(presetState(withLook(base, "Gone", PRESETS[0]!.look), PRESETS)).toEqual({ name: "Gone", modified: true });
  });

  it("is null with no label, a null label or the pinned empty label", () => {
    expect(presetState(withLook(base, undefined, PRESETS[0]!.look), PRESETS)).toBeNull();
    expect(presetState(withLook(base, null, PRESETS[0]!.look), PRESETS)).toBeNull();
    expect(presetState(withLook(base, "", PRESETS[0]!.look), PRESETS)).toBeNull();
  });

  it("heads the strip with the name, the modified mark or no preset", () => {
    expect(presetHeading({ name: "Rich", modified: false })).toBe("Rich");
    expect(presetHeading({ name: "Rich", modified: true })).toBe("Rich (modified)");
    expect(presetHeading(null)).toBe("No preset");
  });
});

describe("saveNameCheck", () => {
  // Standard and Soft are built-ins; Rich stands for a user preset here.
  const BUILTINS = 2;

  it("refuses a built-in name, whatever its case, with Save disabled", () => {
    for (const n of ["Soft", "soft", "  STANDARD "]) {
      const c = saveNameCheck(n, PRESETS, BUILTINS);
      expect(c.blocked).toBe(true);
      expect(c.note).toMatch(/is a built-in preset; choose another name$/);
      expect(c.note).not.toContain("replaces");
    }
    expect(saveNameCheck("soft", PRESETS, BUILTINS).note).toBe("Soft is a built-in preset; choose another name");
  });

  it("says a user preset of the name is replaced, and allows it", () => {
    expect(saveNameCheck("rich", PRESETS, BUILTINS)).toEqual({
      note: "Saving replaces your preset of this name.",
      blocked: false,
    });
  });

  it("allows a new name silently and blocks an empty one", () => {
    expect(saveNameCheck("Mine", PRESETS, BUILTINS)).toEqual({ note: null, blocked: false });
    expect(saveNameCheck("   ", PRESETS, BUILTINS)).toEqual({ note: null, blocked: true });
  });
});

describe("nextPreset", () => {
  it("steps forward and wraps to the first", () => {
    expect(nextPreset(PRESETS, "Standard", 1).name).toBe("Soft");
    expect(nextPreset(PRESETS, "Rich", 1).name).toBe("Standard");
  });

  it("steps back and wraps to the last", () => {
    expect(nextPreset(PRESETS, "Soft", -1).name).toBe("Standard");
    expect(nextPreset(PRESETS, "Standard", -1).name).toBe("Rich");
  });

  it("starts at the first from no preset or an unknown one", () => {
    expect(nextPreset(PRESETS, null, 1).name).toBe("Standard");
    expect(nextPreset(PRESETS, null, -1).name).toBe("Standard");
    expect(nextPreset(PRESETS, "Gone", 1).name).toBe("Standard");
  });
});

describe("presetCycleStep", () => {
  const key = (k: string, extra: Partial<KeyboardEvent> = {}) => ({
    key: k,
    shiftKey: false,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    ...extra,
  });

  it("P cycles forward and Shift+P back", () => {
    expect(presetCycleStep(key("p"), false)).toBe(1);
    expect(presetCycleStep(key("P", { shiftKey: true }), false)).toBe(-1);
  });

  it("does nothing while typing in a text field", () => {
    expect(presetCycleStep(key("p"), true)).toBeNull();
    expect(presetCycleStep(key("P", { shiftKey: true }), true)).toBeNull();
  });

  it("leaves Cmd, Ctrl and Alt combinations and other keys alone", () => {
    expect(presetCycleStep(key("p", { metaKey: true }), false)).toBeNull();
    expect(presetCycleStep(key("p", { ctrlKey: true }), false)).toBeNull();
    expect(presetCycleStep(key("p", { altKey: true }), false)).toBeNull();
    // [ and ] move between frames.
    expect(presetCycleStep(key("]"), false)).toBeNull();
    expect(presetCycleStep(key("["), false)).toBeNull();
  });
});

describe("the frame screen's preset strip", () => {
  const props = (over: Partial<PresetStripProps> = {}): PresetStripProps => ({
    presets: PRESETS,
    urls: { Soft: "blob:soft", Rich: null },
    state: { name: "Rich", modified: true },
    disabled: false,
    onApply: () => undefined,
    onCycle: () => undefined,
    ...over,
  });

  it("renders one item per preset and the heading", () => {
    const html = renderToStaticMarkup(createElement(PresetStrip, props()));
    expect(html.match(/data-preset=/g)).toHaveLength(PRESETS.length);
    for (const p of PRESETS) expect(html).toContain(`>${p.name}<`);
    expect(html).toContain("Rich (modified)");
    expect(html).toContain('src="blob:soft"');
  });

  it("clicking an item applies that preset", () => {
    const applied: string[] = [];
    const tree = PresetStrip(props({ onApply: (n) => applied.push(n) }));
    const soft = buttons(tree).find((b) => b.props["data-preset"] === "Soft");
    (soft?.props.onClick as () => void)();
    expect(applied).toEqual(["Soft"]);
  });

  it("the cycle buttons step forward and back and name the keys", () => {
    const steps: number[] = [];
    const tree = PresetStrip(props({ onCycle: (s) => steps.push(s) }));
    const next = buttons(tree).find((b) => text(b) === "Next preset");
    const prev = buttons(tree).find((b) => text(b) === "Previous preset");
    (next?.props.onClick as () => void)();
    (prev?.props.onClick as () => void)();
    expect(steps).toEqual([1, -1]);
    expect(String(next?.props.title)).toContain("P");
    expect(String(prev?.props.title)).toContain("Shift+P");
  });

  it("disables every item when the frame cannot be edited", () => {
    const tree = PresetStrip(props({ disabled: true }));
    expect(buttons(tree).every((b) => b.props.disabled === true)).toBe(true);
  });
});

describe("the roll preset picker", () => {
  it("applies to frames without their own look and asks nothing when there are none", async () => {
    const applied: Array<[string, string]> = [];
    const out = await chooseRollPreset("Rich", {
      framesWithOwnLook: async () => [],
      apply: async (name, scope) => {
        applied.push([name, scope]);
        return true;
      },
    });
    expect(out).toEqual({ kind: "applied" });
    expect(applied).toEqual([["Rich", "frames_without_own_look"]]);
  });

  it("asks when some frames have their own look, applying nothing yet", async () => {
    const applied: unknown[] = [];
    const out = await chooseRollPreset("Rich", {
      framesWithOwnLook: async () => [1, 4, 5],
      apply: async (...a) => {
        applied.push(a);
        return true;
      },
    });
    expect(out).toEqual({ kind: "ask", frames: [1, 4, 5] });
    expect(applied).toEqual([]);
  });

  it("stops when the frames could not be listed", async () => {
    const out = await chooseRollPreset("Rich", { framesWithOwnLook: async () => null, apply: async () => true });
    expect(out).toEqual({ kind: "failed" });
  });

  it("words the question", () => {
    expect(rollPresetQuestion(3, "Rich")).toBe("3 frames have their own look. Apply Rich to:");
    expect(rollPresetQuestion(1, "Rich")).toBe("1 frame has its own look. Apply Rich to:");
  });

  it("the prompt's buttons send the matching scope, and Cancel cancels", () => {
    expect(ROLL_PRESET_CHOICES.map((c) => [c.label, c.scope])).toEqual([
      ["All frames", "all_frames"],
      ["Only frames without their own look", "frames_without_own_look"],
    ]);
    const chosen: string[] = [];
    let cancelled = 0;
    const tree = RollPresetPrompt({
      name: "Rich",
      count: 3,
      onChoose: (s) => chosen.push(s),
      onCancel: () => (cancelled += 1),
    });
    const bs = buttons(tree);
    for (const b of bs) (b.props.onClick as () => void)();
    expect(chosen).toEqual(["all_frames", "frames_without_own_look"]);
    expect(cancelled).toBe(1);
    expect(bs.map(text)).toEqual(["All frames", "Only frames without their own look", "Cancel"]);
    expect(text(tree)).toContain("3 frames have their own look. Apply Rich to:");
  });
});

describe("export choices", () => {
  it("maps Original and Web/Instagram to the request's web field", () => {
    expect(webOutput(true, false)).toBe("none");
    expect(webOutput(true, true)).toBe("also");
    expect(webOutput(false, true)).toBe("only");
  });

  it("has nothing to export with neither ticked", () => {
    expect(webOutput(false, false)).toBeNull();
  });
});

describe("preset previews", () => {
  const view = decodeFrameView(frameViewFixture);

  it("the version follows the frame's corrections and the preset's look, not its own look", () => {
    const look = PRESETS[1]!.look;
    const v = previewVersion(view, look);
    const lookOnly = structuredClone(view);
    lookOnly.settings.print = { ...lookOnly.settings.print, grade: 150, toe: 2, saturation: 0.5, preset: "Rich" };
    expect(previewVersion(lookOnly, look)).toBe(v);
    const density = structuredClone(view);
    density.settings.print.exposure += 0.1;
    expect(previewVersion(density, look)).not.toBe(v);
    expect(previewVersion(view, { ...look, grade: 96 })).not.toBe(v);
  });

  it("lookOf reads the five look values, saturation 1 when absent", () => {
    const s = structuredClone(base);
    delete s.print.saturation;
    expect(lookOf(s)).toEqual({ grade: 115, toe: 6, shoulder: 3, black_point_compensation: false, saturation: 1 });
  });

  function cache() {
    const fetched: Array<[number, string]> = [];
    const revoked: string[] = [];
    const changes: Array<[number, string, string | null]> = [];
    let n = 0;
    const releases: Array<() => void> = [];
    const c = new PresetPreviewCache({
      fetch: (frame, name) => {
        fetched.push([frame, name]);
        return new Promise((r) => releases.push(() => r(new ArrayBuffer(1))));
      },
      makeUrl: () => `blob:${(n += 1)}`,
      revoke: (u) => revoked.push(u),
      onChange: (frame, name, url) => changes.push([frame, name, url]),
    });
    const flush = async () => {
      while (releases.length > 0) {
        releases.shift()?.();
        await new Promise((r) => setTimeout(r, 0));
      }
    };
    return { c, fetched, revoked, changes, flush };
  }

  it("fetches once per frame, preset and version", async () => {
    const { c, fetched, changes, flush } = cache();
    c.request(0, "Soft", "v1");
    c.request(0, "Soft", "v1");
    await flush();
    c.request(0, "Soft", "v1");
    await flush();
    expect(fetched).toEqual([[0, "Soft"]]);
    expect(changes).toEqual([[0, "Soft", "blob:1"]]);
  });

  it("fetches again when the version changes and revokes the old URL", async () => {
    const { c, fetched, revoked, changes, flush } = cache();
    c.request(2, "Rich", "v1");
    await flush();
    c.request(2, "Rich", "v2");
    await flush();
    expect(fetched).toEqual([
      [2, "Rich"],
      [2, "Rich"],
    ]);
    expect(changes.at(-1)).toEqual([2, "Rich", "blob:2"]);
    expect(revoked).toEqual(["blob:1"]);
  });

  it("forgets everything on reset", async () => {
    const { c, fetched, revoked, flush } = cache();
    c.request(0, "Soft", "v1");
    await flush();
    c.reset();
    expect(revoked).toEqual(["blob:1"]);
    c.request(0, "Soft", "v1");
    await flush();
    expect(fetched).toHaveLength(2);
  });
});
