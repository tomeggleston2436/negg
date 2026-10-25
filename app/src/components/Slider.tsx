// SPDX-License-Identifier: Apache-2.0
// One slider with numeric entry. Dragging (pointer or arrow keys) reports
// `onDrag` for each step; releasing the pointer or key, or entering a number,
// reports `onCommit` once. The thumb follows the drag even when the caller
// does not preview. A value outside the UI range (from a sidecar) is shown
// clamped on the track and unchanged in the number field, and is never
// rewritten until the user changes it (Ruling 78).

import { useEffect, useId, useRef, useState } from "react";
import { type SliderSpec, clampToSpec, enteredValue, formatValue, outOfRange, sliderPosition } from "../sliders";

export interface SliderProps {
  spec: Pick<SliderSpec, "id" | "label" | "hint" | "min" | "max" | "step">;
  value: number;
  disabled?: boolean;
  onDrag?: (v: number) => void;
  onCommit: (v: number) => void;
}

export function Slider({ spec, value, disabled, onDrag, onCommit }: SliderProps) {
  const id = useId();
  const [text, setText] = useState(formatValue(spec, value));
  const [editing, setEditing] = useState(false);
  const [live, setLive] = useState<number | null>(null);
  const dragging = useRef<number | null>(null);
  const focusText = useRef("");
  const cancelled = useRef(false);

  useEffect(() => {
    if (!editing) setText(formatValue(spec, value));
    setLive(null);
  }, [value, editing, spec]);

  const finishDrag = () => {
    const v = dragging.current;
    dragging.current = null;
    setLive(null);
    if (v !== null) onCommit(v);
  };

  const enter = () => {
    const typed = text;
    const shown = focusText.current;
    // Whatever happens, the field now counts as showing what it shows.
    focusText.current = typed;
    if (cancelled.current) {
      cancelled.current = false;
      setEditing(false);
      setText(formatValue(spec, value));
      return;
    }
    const v = enteredValue(spec, typed, shown, value);
    if (v === null) {
      if (typed.trim() !== shown.trim()) setText(formatValue(spec, value));
      return;
    }
    setText(formatValue(spec, v));
    onCommit(v);
  };

  const outside = outOfRange(spec, value);
  return (
    <div className="slider">
      <label htmlFor={`${id}-range`} className="slider-label" title={spec.hint}>
        {spec.label}
      </label>
      <input
        id={`${id}-range`}
        type="range"
        min={spec.min}
        max={spec.max}
        step={spec.step}
        value={live ?? sliderPosition(spec, value)}
        disabled={disabled}
        aria-describedby={`${id}-hint`}
        onChange={(e) => {
          const v = clampToSpec(spec, Number(e.currentTarget.value));
          dragging.current = v;
          setLive(v);
          setText(formatValue(spec, v));
          onDrag?.(v);
        }}
        onPointerUp={finishDrag}
        onKeyUp={finishDrag}
        onBlur={finishDrag}
      />
      <input
        className={outside ? "slider-number outside" : "slider-number"}
        type="text"
        inputMode="decimal"
        aria-label={`${spec.label} value`}
        title={outside ? `Outside the slider range ${spec.min} to ${spec.max}` : undefined}
        value={text}
        disabled={disabled}
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
            cancelled.current = true;
            e.currentTarget.blur();
          }
        }}
      />
      <span id={`${id}-hint`} className="visually-hidden">
        {spec.hint}
      </span>
    </div>
  );
}
