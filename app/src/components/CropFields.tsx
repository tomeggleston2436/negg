// SPDX-License-Identifier: Apache-2.0
// Numeric crop entry: left, top, width and height as percentages of the
// source frame (before any quarter turn or flip). This is also the keyboard
// route to the crop. A field commits on Enter or when left, and only when its
// text changed since it took focus.

import { useEffect, useId, useRef, useState } from "react";
import { type CropField, type NormRect, WHOLE_FRAME, cropWithField, formatPercent, parseCropText } from "../tiles";

const FIELDS: ReadonlyArray<[CropField, string]> = [
  ["x", "Left"],
  ["y", "Top"],
  ["width", "Width"],
  ["height", "Height"],
];

export interface CropFieldsProps {
  crop: NormRect | null;
  disabled: boolean;
  /** Prefix for the accessible names, e.g. "Frame" or "Roll". */
  label: string;
  /** What the rect is, for the accessible names: "crop" (default) or "analysis area". */
  noun?: string;
  onCommit: (crop: NormRect) => void;
}

export function CropFields({ crop, disabled, label, noun = "crop", onCommit }: CropFieldsProps) {
  return (
    <div className="crop-fields" role="group" aria-label={`${label} ${noun} in percent of the frame`}>
      {FIELDS.map(([field, name]) => (
        <CropField
          key={field}
          field={field}
          name={name}
          label={label}
          noun={noun}
          crop={crop}
          disabled={disabled}
          onCommit={onCommit}
        />
      ))}
    </div>
  );
}

function CropField(props: {
  field: CropField;
  name: string;
  label: string;
  noun: string;
  crop: NormRect | null;
  disabled: boolean;
  onCommit: (crop: NormRect) => void;
}) {
  const { field, crop } = props;
  const id = useId();
  const shownValue = formatPercent((crop ?? WHOLE_FRAME)[field]);
  const [text, setText] = useState(shownValue);
  const [editing, setEditing] = useState(false);
  const focusText = useRef("");

  useEffect(() => {
    if (!editing) setText(shownValue);
  }, [shownValue, editing]);

  const enter = () => {
    const typed = text;
    const shown = focusText.current;
    focusText.current = typed;
    if (typed.trim() === shown.trim()) return;
    const v = parseCropText(typed);
    if (v === null) {
      setText(shownValue);
      return;
    }
    const next = cropWithField(crop, field, v);
    setText(formatPercent(next[field]));
    props.onCommit(next);
  };

  return (
    <label className="crop-field" htmlFor={id}>
      <span>{props.name} %</span>
      <input
        id={id}
        type="text"
        inputMode="decimal"
        aria-label={`${props.label} ${props.noun} ${props.name.toLowerCase()} in percent`}
        value={text}
        disabled={props.disabled}
        onFocus={(e) => {
          focusText.current = e.currentTarget.value;
          setEditing(true);
        }}
        onChange={(e) => setText(e.currentTarget.value)}
        onBlur={() => {
          enter();
          setEditing(false);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            enter();
          } else if (e.key === "Escape") {
            e.stopPropagation();
            focusText.current = text;
            setText(shownValue);
            e.currentTarget.blur();
          }
        }}
      />
    </label>
  );
}
