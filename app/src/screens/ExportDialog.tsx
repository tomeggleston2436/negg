// SPDX-License-Identifier: Apache-2.0
// The export dialog: scope, what to export (the original, a Web/Instagram
// copy or both), format, folder and output space. Format and output space are
// the original's; the web copy is always an sRGB JPEG. The output space is a
// roll setting, so a change is committed as a roll edit before the export;
// exports then run as the CLI would and are followed on the
// batch screen.

import { type KeyboardEvent as ReactKeyboardEvent, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { type ExportFormat, type ExportRequest, type OutputSpace, OUTPUT_SPACES } from "../ipc/types";
import { errorText, useStore } from "../state/store";
import { describe, tabStops } from "../focus";
import { OUTPUT_SPACE_LABELS } from "../components/RollSettingsPanel";
import { webOutput } from "../presets";

type ScopeChoice = "frame" | "selected" | "roll";

export function ExportDialog() {
  const { state, dispatch, actions } = useStore();
  const { roll } = state;
  const onFrame = state.screen === "frame" && state.current !== null;
  const [scope, setScope] = useState<ScopeChoice>(onFrame ? "frame" : state.selected.length > 0 ? "selected" : "roll");
  const [format, setFormat] = useState<ExportFormat>("tiff16");
  const [outDir, setOutDir] = useState<string | null>(null);
  const [space, setSpace] = useState<OutputSpace>(roll?.roll.shared.colour.output_space ?? "srgb");
  const [busy, setBusy] = useState(false);
  const [original, setOriginal] = useState(true);
  const [webCopy, setWebCopy] = useState(false);
  const web = webOutput(original, webCopy);
  const firstRef = useRef<HTMLInputElement>(null);

  const modalRef = useRef<HTMLDivElement>(null);

  // Focus moves into the dialog and returns to where it was on close.
  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    firstRef.current?.focus();
    return () => before?.focus();
  }, []);

  // Escape still closes the dialog when focus has fallen to the page body
  // (keys there never reach the dialog's own handler).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (modalRef.current?.contains(document.activeElement)) return;
      e.preventDefault();
      dispatch({ type: "export/dialog", open: false });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dispatch]);

  // The dialog keeps the keyboard: Escape closes it, Tab cycles inside it,
  // and no key reaches the screen behind (the frame screen also ignores keys
  // while the dialog is open).
  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      dispatch({ type: "export/dialog", open: false });
      return;
    }
    if (e.key !== "Tab" || !modalRef.current) return;
    const candidates = Array.from(
      modalRef.current.querySelectorAll<HTMLElement>("button, input, select, textarea, [tabindex]"),
    );
    const focusable = tabStops(candidates.map(describe)).flatMap((i) => candidates[i] ?? []);
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) return;
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  if (!roll) return null;

  const close = () => dispatch({ type: "export/dialog", open: false });

  const chooseFolder = async () => {
    try {
      const dir = await open({ directory: true, multiple: false, title: "Export to folder" });
      if (typeof dir === "string") setOutDir(dir);
    } catch (e) {
      dispatch({ type: "error/set", error: `Could not show the folder picker: ${errorText(e)}` });
    }
  };

  const submit = async () => {
    if (web === null) return;
    let request: ExportRequest;
    let total: number;
    if (scope === "frame" && state.current !== null) {
      request = { scope: { kind: "frame", frame: state.current }, format, out_dir: outDir, web };
      total = 1;
    } else if (scope === "selected") {
      request = { scope: { kind: "frames", frames: [...state.selected] }, format, out_dir: outDir, web };
      total = state.selected.length;
    } else {
      request = { scope: { kind: "roll" }, format, out_dir: outDir, web };
      total = roll.frames.length;
    }
    setBusy(true);
    // The output space is the original's; a web-only export leaves it alone.
    if (original && space !== roll.roll.shared.colour.output_space) {
      const next = structuredClone(roll.roll);
      next.shared.colour.output_space = space;
      const ok = await actions.commitRoll(next);
      if (!ok) {
        setBusy(false);
        return;
      }
    }
    dispatch({ type: "screen/show", screen: "batch" });
    void actions.runExport(request, total);
  };

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        // A click on the backdrop keeps focus in the dialog.
        if (e.target === e.currentTarget) e.preventDefault();
      }}
    >
      <div
        ref={modalRef}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="export-title"
        onKeyDown={onKeyDown}
      >
        <h2 id="export-title">Export</h2>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <fieldset>
            <legend>Frames</legend>
            <label className="check">
              <input
                ref={onFrame ? firstRef : undefined}
                type="radio"
                name="scope"
                checked={scope === "frame"}
                disabled={!onFrame}
                onChange={() => setScope("frame")}
              />
              This frame{onFrame && state.current !== null ? ` (${state.current + 1})` : ""}
            </label>
            <label className="check">
              <input
                type="radio"
                name="scope"
                checked={scope === "selected"}
                disabled={state.selected.length === 0}
                onChange={() => setScope("selected")}
              />
              Selected frames ({state.selected.length})
            </label>
            <label className="check">
              <input
                ref={onFrame ? undefined : firstRef}
                type="radio"
                name="scope"
                checked={scope === "roll"}
                onChange={() => setScope("roll")}
              />
              Whole roll ({roll.frames.length})
            </label>
          </fieldset>
          <fieldset>
            <legend>What to export</legend>
            <label className="check">
              <input type="checkbox" checked={original} onChange={(e) => setOriginal(e.currentTarget.checked)} />
              Original
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={webCopy}
                aria-describedby="export-web-help"
                onChange={(e) => setWebCopy(e.currentTarget.checked)}
              />
              Web/Instagram
            </label>
            <p className="note" id="export-web-help">
              JPEG, sRGB, 2048 px long edge, in a web folder
            </p>
            {web === null && (
              <p className="note warn" role="status">
                Tick at least one.
              </p>
            )}
          </fieldset>
          <fieldset disabled={!original}>
            <legend>Format of the original</legend>
            <label className="check">
              <input type="radio" name="format" checked={format === "tiff16"} onChange={() => setFormat("tiff16")} />
              TIFF, 16-bit
            </label>
            <label className="check">
              <input type="radio" name="format" checked={format === "jpeg"} onChange={() => setFormat("jpeg")} />
              JPEG
            </label>
          </fieldset>
          <fieldset>
            <legend>Folder</legend>
            <p className="note" title={outDir ?? undefined}>
              {outDir ?? "Beside the source files"}
            </p>
            <div className="button-row">
              <button type="button" onClick={() => void chooseFolder()}>
                Choose folder
              </button>
              <button type="button" disabled={outDir === null} onClick={() => setOutDir(null)}>
                Beside the sources
              </button>
            </div>
          </fieldset>
          <label className="field">
            <span>Output space of the original (a roll setting)</span>
            <select
              value={space}
              disabled={!original}
              onChange={(e) => setSpace(e.currentTarget.value as OutputSpace)}
            >
              {OUTPUT_SPACES.map((sp) => (
                <option key={sp} value={sp}>
                  {OUTPUT_SPACE_LABELS[sp]}
                </option>
              ))}
            </select>
          </label>
          <div className="button-row end">
            <button type="button" onClick={close}>
              Cancel
            </button>
            <button type="submit" className="primary" disabled={busy || web === null}>
              Export
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
