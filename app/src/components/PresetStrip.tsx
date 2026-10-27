// SPDX-License-Identifier: Apache-2.0
// The frame screen's tone presets: a strip of small previews of this frame
// as each preset would print it (clicking one applies it to the frame), the
// previous and next buttons (P and Shift+P), the heading with the
// preset the look came from ("Rich" or "Rich (modified)"), and
// "Save as preset" with an inline name field.

import { useState } from "react";
import type { Preset } from "../ipc/types";
import { type PresetState, presetHeading, saveNameCheck } from "../presets";

export interface PresetStripProps {
  presets: readonly Preset[];
  /** Preview URL per preset name: absent while loading, null when it failed. */
  urls: Readonly<Record<string, string | null | undefined>>;
  state: PresetState | null;
  disabled: boolean;
  onApply: (name: string) => void;
  onCycle: (step: 1 | -1) => void;
}

/** Hookless, so a test can call it and press its buttons. */
export function PresetStrip({ presets, urls, state, disabled, onApply, onCycle }: PresetStripProps) {
  const none = presets.length === 0;
  return (
    <section className="presets" aria-labelledby="preset-heading">
      <h2 id="preset-heading">Preset</h2>
      <div className="preset-head">
        <span className="preset-current" aria-live="polite">
          {presetHeading(state)}
        </span>
        <div className="button-row">
          <button
            type="button"
            disabled={disabled || none}
            onClick={() => onCycle(-1)}
            title="Apply the previous preset (Shift+P)"
          >
            Previous preset
          </button>
          <button
            type="button"
            disabled={disabled || none}
            onClick={() => onCycle(1)}
            title="Apply the next preset (P)"
          >
            Next preset
          </button>
        </div>
      </div>
      <ul className="preset-strip">
        {presets.map((p) => {
          const url = urls[p.name];
          const active = state?.name === p.name;
          return (
            <li key={p.name}>
              <button
                type="button"
                className={active ? "preset-item active" : "preset-item"}
                data-preset={p.name}
                aria-pressed={active}
                disabled={disabled}
                onClick={() => onApply(p.name)}
                title={`Apply ${p.name} to this frame`}
              >
                <span className="preset-image">
                  {url ? <img src={url} alt="" /> : <span className="thumb-placeholder">{url === null ? "No preview" : ""}</span>}
                </span>
                <span className="preset-name">{p.name}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

export interface SavePresetProps {
  presets: readonly Preset[];
  /** How many of `presets` are built-ins (they come first). */
  builtins: number;
  disabled: boolean;
  /** Saves the current look under `name`; resolves to why it was refused, or null. */
  onSave: (name: string) => Promise<string | null>;
}

/** "Save as preset": the name is typed inline (no browser dialog). */
export function SavePreset({ presets, builtins, disabled, onSave }: SavePresetProps) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (!open) {
    return (
      <div className="button-row">
        <button
          type="button"
          disabled={disabled}
          onClick={() => {
            setName("");
            setError(null);
            setOpen(true);
          }}
          title="Save this frame's grade, toe, shoulder, black point compensation and saturation as a preset"
        >
          Save as preset
        </button>
      </div>
    );
  }

  const trimmed = name.trim();
  const check = saveNameCheck(trimmed, presets, builtins);
  const close = () => setOpen(false);
  const save = async () => {
    if (check.blocked || busy) return;
    setBusy(true);
    const refused = await onSave(trimmed);
    setBusy(false);
    if (refused === null) close();
    else setError(refused);
  };

  return (
    <form
      className="save-preset"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <label className="field">
        <span>Preset name</span>
        <input
          type="text"
          value={name}
          autoFocus
          maxLength={64}
          onChange={(e) => {
            setName(e.currentTarget.value);
            setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              close();
            }
          }}
        />
      </label>
      {check.note && <p className="note">{check.note}</p>}
      {error && (
        <p className="note warn" role="alert">
          {error}
        </p>
      )}
      <div className="button-row">
        <button type="button" onClick={close}>
          Cancel
        </button>
        <button type="submit" className="primary" disabled={check.blocked || busy}>
          Save
        </button>
      </div>
    </form>
  );
}
