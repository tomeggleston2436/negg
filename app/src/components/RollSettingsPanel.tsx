// SPDX-License-Identifier: Apache-2.0
// The roll settings panel: edits RollSettings (shared settings, pool flags and
// profiles) with commit_roll (Ruling 79). There is no roll preview command, so
// a drag only moves the control and the release commits.

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { type OutputSpace, OUTPUT_SPACES, type RollSettings, emptyOverrides, resolve } from "../ipc/types";
import { SLIDERS, turn } from "../sliders";
import { Slider } from "./Slider";
import { CropFields } from "./CropFields";

export const OUTPUT_SPACE_LABELS: Record<OutputSpace, string> = {
  srgb: "sRGB",
  adobe_rgb: "Adobe RGB (1998)",
  display_p3: "Display P3",
  pro_photo: "ProPhoto RGB",
  rec2020: "Rec. 2020",
};

export interface RollSettingsPanelProps {
  roll: RollSettings;
  disabled: boolean;
  onCommit: (roll: RollSettings) => void;
  onError: (e: unknown) => void;
}

export function RollSettingsPanel({ roll, disabled, onCommit, onError }: RollSettingsPanelProps) {
  const [draft, setDraft] = useState<RollSettings>(roll);
  useEffect(() => setDraft(roll), [roll]);
  const shared = draft.shared;
  const none = emptyOverrides();

  const withShared = (patch: (s: RollSettings["shared"]) => RollSettings["shared"]): RollSettings => ({
    ...draft,
    shared: patch(structuredClone(draft.shared)),
  });

  const pickProfile = async (which: "rig" | "film") => {
    let path: string | string[] | null;
    try {
      path = await open({
        multiple: false,
        directory: false,
        title: which === "rig" ? "Choose a rig profile" : "Choose a film profile",
        filters: [{ name: "Profile", extensions: ["toml"] }],
      });
    } catch (e) {
      onError(e);
      return;
    }
    if (typeof path === "string") {
      onCommit(withShared((s) => ({ ...s, profiles: { ...s.profiles, [which]: path } })));
    }
  };

  const geometry = shared.geometry;
  const withGeometry = (patch: Partial<RollSettings["shared"]["geometry"]>): RollSettings =>
    withShared((s) => ({ ...s, geometry: { ...s.geometry, ...patch } }));
  const buffer = SLIDERS.find((sp) => sp.id === "analysis_buffer");

  return (
    <div className="panel-body">
      <section aria-labelledby="roll-print-heading">
        <h2 id="roll-print-heading">Roll print settings</h2>
        {SLIDERS.filter((sp) => sp.id !== "analysis_buffer").map((spec) => (
          <Slider
            key={spec.id}
            spec={spec}
            value={spec.get(none, shared)}
            disabled={disabled}
            onDrag={(v) => setDraft(withShared((s) => resolve(s, spec.set(none, v, s))))}
            onCommit={(v) => onCommit(withShared((s) => resolve(s, spec.set(none, v, s))))}
          />
        ))}
        <label className="check">
          <input
            type="checkbox"
            checked={shared.print.black_point_compensation}
            disabled={disabled}
            onChange={(e) => {
              const on = e.currentTarget.checked;
              onCommit(withShared((s) => ({ ...s, print: { ...s.print, black_point_compensation: on } })));
            }}
          />
          Black point compensation
        </label>
      </section>

      <section aria-labelledby="roll-geometry-heading">
        <h2 id="roll-geometry-heading">Roll geometry</h2>
        <p className="note">Shared by every frame without its own geometry.</p>
        <div className="button-row">
          <button
            type="button"
            disabled={disabled}
            onClick={() => onCommit(withGeometry({ rotate_degrees: turn(geometry.rotate_degrees, -1) }))}
          >
            Rotate left
          </button>
          <button
            type="button"
            disabled={disabled}
            onClick={() => onCommit(withGeometry({ rotate_degrees: turn(geometry.rotate_degrees, 1) }))}
          >
            Rotate right
          </button>
        </div>
        <label className="check">
          <input
            type="checkbox"
            checked={geometry.flip_horizontal}
            disabled={disabled}
            onChange={(e) => {
              const on = e.currentTarget.checked;
              onCommit(withGeometry({ flip_horizontal: on }));
            }}
          />
          Flip horizontally
        </label>
        <CropFields
          crop={geometry.crop}
          disabled={disabled}
          label="Roll"
          onCommit={(crop) => onCommit(withGeometry({ crop }))}
        />
        <div className="button-row">
          <button
            type="button"
            disabled={disabled || geometry.crop === null}
            onClick={() => onCommit(withGeometry({ crop: null }))}
          >
            Clear roll crop
          </button>
        </div>
        {buffer && (
          <Slider
            spec={buffer}
            value={geometry.analysis_buffer}
            disabled={disabled || (geometry.analysis_area ?? geometry.crop) === null}
            onDrag={(v) => setDraft(withGeometry({ analysis_buffer: v }))}
            onCommit={(v) => onCommit(withGeometry({ analysis_buffer: v }))}
          />
        )}
      </section>

      <section aria-labelledby="roll-colour-heading">
        <h2 id="roll-colour-heading">Colour</h2>
        <label className="check">
          <input
            type="checkbox"
            checked={shared.colour.auto_colour}
            disabled={disabled}
            onChange={(e) => {
              const on = e.currentTarget.checked;
              onCommit(withShared((s) => ({ ...s, colour: { ...s.colour, auto_colour: on } })));
            }}
          />
          Automatic colour
        </label>
        <label className="field">
          <span>Output space</span>
          <select
            value={shared.colour.output_space}
            disabled={disabled}
            onChange={(e) => {
              const space = e.currentTarget.value as OutputSpace;
              onCommit(withShared((s) => ({ ...s, colour: { ...s.colour, output_space: space } })));
            }}
          >
            {OUTPUT_SPACES.map((sp) => (
              <option key={sp} value={sp}>
                {OUTPUT_SPACE_LABELS[sp]}
              </option>
            ))}
          </select>
        </label>
      </section>

      <section aria-labelledby="roll-pool-heading">
        <h2 id="roll-pool-heading">Roll pooling</h2>
        {(
          [
            ["pool_film_base", "Pool film base"],
            ["pool_gamma", "Pool gamma"],
            ["pool_colour", "Pool colour cast"],
          ] as const
        ).map(([key, label]) => (
          <label className="check" key={key}>
            <input
              type="checkbox"
              checked={draft[key]}
              disabled={disabled}
              onChange={(e) => onCommit({ ...draft, [key]: e.currentTarget.checked })}
            />
            {label}
          </label>
        ))}
      </section>

      <section aria-labelledby="roll-profiles-heading">
        <h2 id="roll-profiles-heading">Profiles</h2>
        {(["rig", "film"] as const).map((which) => {
          const path = shared.profiles[which];
          const name = which === "rig" ? "Rig profile" : "Film profile";
          return (
            <div className="profile" key={which}>
              <span className="profile-name">{name}</span>
              <span className="profile-path" title={path ?? undefined}>
                {path ? basename(path) : "None"}
              </span>
              <div className="button-row">
                <button type="button" disabled={disabled} onClick={() => void pickProfile(which)}>
                  Choose
                </button>
                <button
                  type="button"
                  disabled={disabled || path === null}
                  aria-label={`Clear ${name.toLowerCase()}`}
                  onClick={() => onCommit(withShared((s) => ({ ...s, profiles: { ...s.profiles, [which]: null } })))}
                >
                  Clear
                </button>
              </div>
            </div>
          );
        })}
      </section>
    </div>
  );
}

export function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}
