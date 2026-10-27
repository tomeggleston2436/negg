// SPDX-License-Identifier: Apache-2.0
// The question asked when a preset is applied to the roll and some frames
// have a look of their own (the spec's "Roll preset vs frames with their own
// look"): apply it to all frames (their look overrides are cleared) or only
// to the frames without one. Inline in the roll panel, not a browser dialog.

import type { RollPresetScope } from "../ipc/types";
import { ROLL_PRESET_CHOICES, rollPresetQuestion } from "../presets";

export interface RollPresetPromptProps {
  name: string;
  /** Frames with a look of their own. */
  count: number;
  onChoose: (scope: RollPresetScope) => void;
  onCancel: () => void;
}

/** Hookless, so a test can call it and press its buttons. */
export function RollPresetPrompt({ name, count, onChoose, onCancel }: RollPresetPromptProps) {
  return (
    <div className="preset-prompt" role="group" aria-label={`Apply ${name} to the roll`}>
      <p>{rollPresetQuestion(count, name)}</p>
      <div className="button-row wrap">
        {ROLL_PRESET_CHOICES.map((c) => (
          <button key={c.scope} type="button" onClick={() => onChoose(c.scope)}>
            {c.label}
          </button>
        ))}
        <button type="button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}
