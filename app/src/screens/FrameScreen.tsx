// SPDX-License-Identifier: Apache-2.0
// The single-frame screen: tiled preview with zoom and pan, the crop and
// analysis area tools, slider panel, lock, undo and redo, scopes and the
// frame's estimates. Before pass 1 reaches the frame it shows the negative
// and the analysis area tool sets the roll's area (Rulings 134 and 136).

import { useEffect, useState } from "react";
import { scopes as fetchScopes } from "../ipc/commands";
import { parseScopes } from "../ipc/scopes";
import type { CropRect, Estimates, FrameOverrides, Rgb } from "../ipc/types";
import { analysing as isAnalysing, awaitingAnalysis, exportBlocked, neighbour, shownOverrides } from "../state/reducer";
import { isTextEntry } from "../focus";
import { errorText, useStore } from "../state/store";
import { analysisArea, effectiveGeometry, hasOwnArea, withGeometry } from "../sliders";
import { SliderPanel } from "../components/SliderPanel";
import { TileView, type Tool, type Zoom } from "../components/TileView";
import { AnalysisAreaPanel } from "../components/AnalysisAreaPanel";
import { Histogram } from "../components/Histogram";
import { Parade } from "../components/Parade";
import { Vectorscope } from "../components/Vectorscope";
import { basename } from "../components/RollSettingsPanel";
import { useRepooling } from "./RollBrowser";

const SCOPES_DELAY_MS = 150;

export function FrameScreen() {
  const { state, dispatch, actions } = useStore();
  const { frame, roll, current, negative } = state;
  const [zoom, setZoom] = useState<Zoom>("fit");
  const [tool, setTool] = useState<Tool>(null);
  const [showNegative, setShowNegative] = useState(false);
  const awaiting = awaitingAnalysis(state);
  const analysing = isAnalysing(state);
  // Edits wait for the analysis (commit_frame is refused before it).
  const notReady = awaiting || analysing;
  const repooling = frame?.pool_pending === true || roll?.pool_pending === true;
  useRepooling(repooling, current);
  const info = current === null ? undefined : roll?.frames[current];

  // Pass 1 reached the frame shown as a negative: show its positive.
  const reached = info !== undefined && info.status !== "pending";
  useEffect(() => {
    if (reached && frame === null && negative !== null && current !== null) void actions.openFrame(current);
  }, [reached, frame, negative, current, actions]);

  // Scopes follow the session generation, a moment after edits settle.
  useEffect(() => {
    if (current === null || !frame || frame.index !== current) return;
    let cancelled = false;
    const t = window.setTimeout(() => {
      fetchScopes(current)
        .then((buf) => {
          if (!cancelled) dispatch({ type: "scopes/loaded", scopes: parseScopes(buf) });
        })
        .catch(() => {
          // Scopes are advisory; a failure leaves the last ones drawn.
        });
    }, SCOPES_DELAY_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [current, frame, state.generation, dispatch]);

  // Put the tool down, reset the zoom and show the positive when the frame
  // changes.
  useEffect(() => {
    setTool(null);
    setZoom("fit");
    setShowNegative(false);
  }, [current]);

  // The negative asked for after analysis: its own size and pyramid.
  const wantNegative = showNegative && frame !== null && frame.index === current;
  const haveNegative = negative !== null && negative.index === current;
  useEffect(() => {
    if (wantNegative && !haveNegative && current !== null) void actions.loadNegative(current);
  }, [wantNegative, haveNegative, current, actions]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // The export dialog owns the keyboard while it is open.
      if (state.exportDialog) return;
      if (isTextEntry(e.target)) return;
      // Undo and redo are the shell's (App.tsx), on every screen.
      const mod = e.metaKey || e.ctrlKey;
      if (mod || e.altKey) return;
      if (isControl(e.target) && (e.key === "ArrowLeft" || e.key === "ArrowRight")) return;
      if (e.key === "ArrowRight" || e.key === "]") {
        const n = neighbour(state, 1);
        if (n !== null) void actions.openFrame(n);
      } else if (e.key === "ArrowLeft" || e.key === "[") {
        const n = neighbour(state, -1);
        if (n !== null) void actions.openFrame(n);
      } else if (e.key === "Escape") {
        if (tool !== null) setTool(null);
        else dispatch({ type: "screen/show", screen: "roll" });
      } else if (e.key === "f") {
        setZoom("fit");
      } else if (e.key === "1") {
        setZoom(1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [state, actions, dispatch, tool]);

  if (current === null || !roll) return null;
  const overrides = shownOverrides(state);
  const prev = neighbour(state, -1);
  const next = neighbour(state, 1);
  const rollGeometry = roll.roll.shared.geometry;
  // The frame's view once analysed; until then its negative under the roll's
  // geometry, and the area tool sets the roll's area.
  const shown = frame && frame.index === current ? frame : null;
  const geometry = shown && overrides ? effectiveGeometry(overrides, shown.settings) : rollGeometry;
  const frameScope = shown !== null && overrides !== null && !notReady && !shown.read_only;
  const own = shown !== null && overrides !== null && hasOwnArea(overrides, rollGeometry);
  // The area shown is the one edits go to: the frame's once it can be edited
  // (or for a read-only frame, whose panel is disabled), else the roll's.
  const editsFrame = frameScope || (shown?.read_only === true && !notReady);
  const area = analysisArea(editsFrame ? geometry : rollGeometry);
  const areaLabel = editsFrame
    ? own
      ? "Frame analysis area"
      : "Analysis area from the roll"
    : "Roll analysis area";
  const mode = shown === null || showNegative ? "negative" : "positive";
  // Negative tiles always use the NegativeView's size and levels.
  const source = mode === "negative" ? (negative && negative.index === current ? negative : null) : shown;
  const needsArea = (shown?.needs_analysis_area ?? info?.needs_analysis_area) === true;
  const setArea = (a: CropRect | null) => {
    if (frameScope && shown && overrides) actions.commit(withGeometry(overrides, { analysis_area: a }, shown.settings));
    else void actions.setRollArea(a);
  };
  const areaDisabled = shown?.read_only === true && !notReady;

  return (
    <div className="frame-screen">
      <header className="toolbar">
        <button type="button" onClick={() => dispatch({ type: "screen/show", screen: "roll" })}>
          Back to roll
        </button>
        <div className="toolbar-group" role="group" aria-label="Frame navigation">
          <button
            type="button"
            disabled={prev === null}
            onClick={() => prev !== null && void actions.openFrame(prev)}
            title="Previous frame (Left arrow or [)"
          >
            Previous
          </button>
          <span className="toolbar-title">
            Frame {current + 1} of {roll.frames.length}
            {info ? `: ${basename(info.source)}` : ""}
          </span>
          <button
            type="button"
            disabled={next === null}
            onClick={() => next !== null && void actions.openFrame(next)}
            title="Next frame (Right arrow or ])"
          >
            Next
          </button>
        </div>
        <div className="toolbar-group" role="group" aria-label="History">
          <button
            type="button"
            disabled={notReady || !state.history.can_undo}
            onClick={actions.undo}
            title="Undo (Cmd or Ctrl+Z; also on the roll screen)"
          >
            Undo
          </button>
          <button
            type="button"
            disabled={notReady || !state.history.can_redo}
            onClick={actions.redo}
            title="Redo (Shift+Cmd or Ctrl+Z)"
          >
            Redo
          </button>
        </div>
        <div className="toolbar-group" role="group" aria-label="Drawing tool">
          <button
            type="button"
            className="tool-area"
            aria-pressed={tool === "area"}
            disabled={areaDisabled || source === null}
            onClick={() => setTool(tool === "area" ? null : "area")}
            title="Draw the analysis area: the whole film including its clear edge (rebate), without the holder"
          >
            Analysis area
          </button>
          <button
            type="button"
            aria-pressed={tool === "crop"}
            disabled={!frameScope}
            onClick={() => setTool(tool === "crop" ? null : "crop")}
            title={frameScope ? "Draw the output crop" : "The crop is available once the frame is analysed"}
          >
            Crop
          </button>
        </div>
        {shown && (
          <button
            type="button"
            aria-pressed={showNegative}
            onClick={() => setShowNegative(!showNegative)}
            title="Show the negative as scanned instead of the positive"
          >
            Negative
          </button>
        )}
        <div className="toolbar-group" role="group" aria-label="Zoom">
          <button type="button" aria-pressed={zoom === "fit"} onClick={() => setZoom("fit")} title="Fit (F)">
            Fit
          </button>
          <button type="button" aria-pressed={zoom === 1} onClick={() => setZoom(1)} title="Actual pixels (1)">
            100%
          </button>
        </div>
        {awaiting ? (
          <button
            type="button"
            className="primary"
            disabled={state.requesting || state.opening}
            onClick={() => void actions.startAnalysis()}
          >
            Analyse
          </button>
        ) : (
          <button
            type="button"
            disabled={exportBlocked(state)}
            onClick={() => dispatch({ type: "export/dialog", open: true })}
            title={exportBlocked(state) ? "Export waits for the analysis and for an export already running" : undefined}
          >
            Export
          </button>
        )}
      </header>

      {frame?.read_only && (
        <p className="banner warn" role="status">
          This frame's sidecar could not be read or was changed by another program, so edits are disabled to protect
          it. Open the folder again to see what is on disk and edit the frame.
        </p>
      )}
      {roll.pool_warning && (
        <p className="banner warn" role="status">
          {roll.pool_warning}
        </p>
      )}
      {awaiting && (
        <p className="banner" role="status">
          <strong>Set the analysis area, then Analyse.</strong> This is the negative as scanned. Draw the analysis area
          over the whole film including its clear edge (rebate), leaving out only the film holder; it is the roll's and
          does not crop the photo.
        </p>
      )}
      {analysing && (
        <p className="banner" role="status">
          Analysing the roll. Edits are available when analysis finishes.
        </p>
      )}
      {repooling && !analysing && (
        <div className="progress-line" role="status" aria-live="polite">
          <progress />
          <span>Analysing the roll again. The estimates shown update when it finishes.</span>
        </div>
      )}
      {needsArea && (
        <p className="banner warn" role="status">
          This frame needs an analysis area: its Dmax is at the density cap, so the film holder or another opaque area
          is in what was analysed, and it is left out of the roll's gamma and colour pools. Draw the analysis area over
          the whole film including its clear edge (rebate), leaving out only the holder.
        </p>
      )}

      <div className="frame-body">
        <main className="preview" aria-label="Preview">
          {source ? (
            <TileView
              source={source}
              mode={mode}
              generation={state.generation}
              geometry={geometry}
              area={area}
              areaLabel={areaLabel}
              zoom={zoom}
              onZoom={setZoom}
              tool={tool}
              onCropDrag={(crop) => shown && overrides && actions.drag(withGeometry(overrides, { crop }, shown.settings))}
              onCrop={(crop) => shown && overrides && actions.commit(withGeometry(overrides, { crop }, shown.settings))}
              onCropCancel={actions.revertDraft}
              onArea={setArea}
              onError={(e) => dispatch({ type: "error/set", error: errorText(e) })}
            />
          ) : (
            <p className="loading">{info?.status === "pending" ? "Opening the negative" : "Opening frame"}</p>
          )}
        </main>
        <aside className="side-panel" aria-label="Adjustments">
          <div className="panel-body">
            <AnalysisAreaPanel
              scope={editsFrame ? "frame" : "roll"}
              area={area}
              crop={(editsFrame ? geometry : rollGeometry).crop}
              own={own}
              disabled={areaDisabled}
              drawing={tool === "area"}
              onDraw={(on) => setTool(on ? "area" : null)}
              onCommit={setArea}
              onUseRoll={frameScope ? actions.useRollArea : undefined}
              hint={
                areaDisabled
                  ? "This frame is read-only; open the folder again to edit it."
                  : frameScope
                  ? "Set here, it is this frame's own; the roll's area is set on the roll screen."
                  : awaiting
                    ? "Before Analyse this sets the roll's area; once analysed you can set one for this frame."
                    : analysing
                      ? own
                        ? "While the roll is analysed this shows and sets the roll's area; this frame keeps its own."
                        : "While the roll is analysed this shows and sets the roll's area."
                      : undefined
              }
            />
          </div>
          {shown && overrides && (
            <SliderPanel
              key={current}
              overrides={overrides}
              settings={shown.settings}
              disabled={notReady || shown.read_only}
              cropMode={tool === "crop"}
              onCropMode={(on) => setTool(on ? "crop" : null)}
              onDrag={(o: FrameOverrides) => actions.drag(o)}
              onCommit={(o: FrameOverrides) => actions.commit(o)}
              onLock={actions.lock}
              onCropToRoll={() => void actions.frameCropToRoll()}
            />
          )}
          <section className="scopes" aria-labelledby="scopes-heading">
            <h2 id="scopes-heading">Scopes</h2>
            <Histogram scopes={state.scopes} />
            <Parade scopes={state.scopes} />
            <Vectorscope scopes={state.scopes} />
          </section>
          {shown?.estimates && <EstimatesTable estimates={shown.estimates} />}
        </aside>
      </div>
    </div>
  );
}

const DMIN_SOURCE_LABELS: Record<Estimates["dmin_source"], string> = {
  border: "film border",
  percentile: "darkest cells",
  fixed: "fixed",
  roll: "roll pool",
  profile: "film profile",
  roll_min: "roll pool, capped",
};

function rgb(v: Rgb): string {
  return v.map((x) => x.toFixed(3)).join(", ");
}

function EstimatesTable({ estimates: e }: { estimates: Estimates }) {
  return (
    <section aria-labelledby="estimates-heading">
      <h2 id="estimates-heading">Estimates</h2>
      <table className="estimates">
        <tbody>
          <tr>
            <th scope="row">Film base (R, G, B)</th>
            <td>
              {rgb(e.dmin)} <span className="muted">({DMIN_SOURCE_LABELS[e.dmin_source]})</span>
            </td>
          </tr>
          <tr>
            <th scope="row">Dmax</th>
            <td>{rgb(e.dmax)}</td>
          </tr>
          <tr>
            <th scope="row">Gamma</th>
            <td>
              {rgb(e.gamma)} <span className="muted">({e.gamma_source.replace("_", " ")})</span>
            </td>
          </tr>
          <tr>
            <th scope="row">Colour cast</th>
            <td>
              red {e.cast.slope_r.toFixed(3)} / {e.cast.offset_r.toFixed(3)}, blue {e.cast.slope_b.toFixed(3)} /{" "}
              {e.cast.offset_b.toFixed(3)} <span className="muted">({e.cast_source})</span>
            </td>
          </tr>
        </tbody>
      </table>
    </section>
  );
}

function isControl(t: EventTarget | null): boolean {
  return t instanceof HTMLInputElement || t instanceof HTMLSelectElement;
}
