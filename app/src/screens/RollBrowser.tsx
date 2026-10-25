// SPDX-License-Identifier: Apache-2.0
// The roll browser: open a folder, see the negatives and set the roll's
// analysis area before pressing Analyse (Ruling 136), watch the analysis
// progress, pick frames (thumbnail grid with status badges) and edit the roll
// settings. Also the re-pooling indicator's polling hook both screens use
// (Ruling 127).

import { useEffect } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { frameView as fetchFrameView, rollView as fetchRollView } from "../ipc/commands";
import { errorText, useStore } from "../state/store";
import { analysing as isAnalysing, awaitingAnalysis, exportBlocked, historyBlocked } from "../state/reducer";
import { analysisArea } from "../sliders";
import { Thumbnail } from "../components/Thumbnail";
import { RollSettingsPanel, basename } from "../components/RollSettingsPanel";
import { AnalysisAreaPanel, AreaSketch } from "../components/AnalysisAreaPanel";

/** How often views are asked for again while a re-pool is pending. */
const REPOOL_POLL_MS = 400;

/**
 * While a re-pool is pending (Rulings 124 and 127), asks for the roll view
 * again every REPOOL_POLL_MS until it clears, then for the frame view of
 * `frame` (when given) so the estimates shown are on the new pool. The
 * `negg://analysis` event already refreshes the roll; polling covers an
 * event that arrived before the view that announced the re-pool.
 */
export function useRepooling(pending: boolean, frame: number | null): void {
  const { dispatch } = useStore();
  useEffect(() => {
    if (!pending) return;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      try {
        const roll = await fetchRollView();
        if (cancelled) return;
        dispatch({ type: "roll/loaded", roll });
        if (!roll.pool_pending) {
          // A frame pass 1 has not reached has no view yet (its negative is shown).
          if (frame === null || !roll.analysed || roll.frames[frame]?.status === "pending") return;
          const view = await fetchFrameView(frame);
          if (cancelled) return;
          // `committed` names the view's own overrides, so a drag in
          // progress keeps its draft.
          dispatch({ type: "frame/loaded", frame: view, committed: view.overrides });
          if (!view.pool_pending) return;
        }
      } catch (e) {
        if (!cancelled) dispatch({ type: "error/set", error: errorText(e) });
        return;
      }
      if (!cancelled) timer = window.setTimeout(() => void tick(), REPOOL_POLL_MS);
    };
    timer = window.setTimeout(() => void tick(), REPOOL_POLL_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [pending, frame, dispatch]);
}

export function RollBrowser() {
  const { state, dispatch, actions } = useStore();
  const { roll, analysis } = state;
  const awaiting = awaitingAnalysis(state);
  const analysing = isAnalysing(state);
  const repooling = roll?.pool_pending === true;
  useRepooling(repooling, null);
  // The frame the roll's area is drawn on: the one last opened, else the
  // first selected, else the first.
  const sketchFrame = state.current ?? state.selected[0] ?? 0;
  const sketchInfo = roll?.frames[sketchFrame];
  useEffect(() => {
    if (sketchInfo && sketchInfo.status !== "failed") actions.requestNegativeThumb(sketchInfo.index);
  }, [sketchInfo, actions]);
  const needsArea = roll?.frames.filter((f) => f.needs_analysis_area === true).length ?? 0;

  const chooseFolder = async () => {
    let dir: string | string[] | null;
    try {
      dir = await open({ directory: true, multiple: false, title: "Open a roll folder" });
    } catch (e) {
      dispatch({ type: "error/set", error: `Could not show the folder picker: ${errorText(e)}` });
      return;
    }
    if (typeof dir === "string") await actions.openRoll(dir);
  };

  return (
    <div className="roll-screen">
      <header className="toolbar">
        <button
          type="button"
          className={awaiting ? undefined : "primary"}
          onClick={() => void chooseFolder()}
          disabled={state.requesting || state.opening}
        >
          {state.requesting || state.opening ? "Opening" : "Open roll folder"}
        </button>
        {awaiting && (
          <button
            type="button"
            className="primary"
            disabled={state.requesting || state.opening}
            onClick={() => void actions.startAnalysis()}
          >
            Analyse
          </button>
        )}
        {roll && (
          <span className="toolbar-title" title={roll.dir}>
            {roll.dir}
          </span>
        )}
        {roll && (
          <div className="toolbar-group" role="group" aria-label="History">
            <button
              type="button"
              disabled={historyBlocked(state) || !state.history.can_undo}
              onClick={actions.undo}
              title="Undo (Cmd or Ctrl+Z)"
            >
              Undo
            </button>
            <button
              type="button"
              disabled={historyBlocked(state) || !state.history.can_redo}
              onClick={actions.redo}
              title="Redo (Shift+Cmd or Ctrl+Z)"
            >
              Redo
            </button>
          </div>
        )}
        {roll && (
          <div className="toolbar-group" role="group" aria-label="Export">
            <button
              type="button"
              disabled={exportBlocked(state) || state.selected.length === 0}
              onClick={() => dispatch({ type: "export/dialog", open: true })}
            >
              Export selected ({state.selected.length})
            </button>
            <button
              type="button"
              disabled={exportBlocked(state)}
              onClick={() => dispatch({ type: "export/dialog", open: true })}
              title={exportBlocked(state) ? "Export waits for the analysis and for an export already running" : undefined}
            >
              Export
            </button>
            {state.exportProgress && (
              <button type="button" onClick={() => dispatch({ type: "screen/show", screen: "batch" })}>
                Last export
              </button>
            )}
          </div>
        )}
      </header>

      {roll && awaiting && (
        <div className="cta" role="status">
          <div>
            <strong>Set the analysis area, then Analyse</strong>
            <p className="muted">
              These are the negatives as scanned. Draw the analysis area over the whole film including its clear edge
              (rebate), where the film base is measured, leaving out only the film holder. It does not crop the photos.
              Nothing is analysed until you press Analyse.
            </p>
          </div>
          <button
            type="button"
            className="primary"
            disabled={state.requesting || state.opening}
            onClick={() => void actions.startAnalysis()}
          >
            Analyse
          </button>
        </div>
      )}

      {roll && !awaiting && (analysing || analysis.total > 0) && (
        <div className="progress-line" role="status" aria-live="polite">
          <progress max={Math.max(1, analysis.total)} value={analysing ? analysis.done : analysis.total} />
          <span>
            {analysing
              ? `Analysing frame ${Math.min(analysis.done + 1, analysis.total)} of ${analysis.total}`
              : `${roll.frames.length} frames analysed`}
          </span>
        </div>
      )}

      {repooling && !analysing && (
        <div className="progress-line" role="status" aria-live="polite">
          <progress />
          <span>Analysing the roll again. Thumbnails and estimates update when it finishes.</span>
        </div>
      )}

      {needsArea > 0 && !analysing && !repooling && (
        <p className="banner warn" role="status">
          {needsArea === 1 ? "1 frame needs" : `${needsArea} frames need`} an analysis area: the film holder is in what
          was analysed. Set the roll's analysis area over the whole film including its clear edge (rebate), leaving out
          only the holder, or one for the frame.
        </p>
      )}

      {roll?.pool_warning && !analysing && !repooling && (
        <p className="banner warn" role="status">
          {roll.pool_warning}
        </p>
      )}

      <div className="roll-body">
        <main className="roll-grid-wrap" aria-label="Frames">
          {!roll && (
            <div className="empty">
              <p>Open a folder of scanned negatives to start.</p>
              <p className="muted">
                Settings are kept in sidecar files beside each frame; opening a roll writes nothing. Setting an analysis
                area, like any other roll setting, saves the roll settings (roll.negg.json) in the folder.
              </p>
            </div>
          )}
          {roll && (
            <ul className="roll-grid">
              {roll.frames.map((f) => (
                <Thumbnail
                  key={`${roll.dir}:${f.index}`}
                  info={f}
                  stale={state.thumbs[f.index] ?? 0}
                  url={state.thumbUrls[f.index]}
                  onNeed={(stale) => actions.requestThumb(f.index, stale)}
                  negativeUrl={state.negativeUrls[f.index]}
                  awaiting={awaiting}
                  onNeedNegative={() => actions.requestNegativeThumb(f.index)}
                  selected={state.selected.includes(f.index)}
                  current={state.current === f.index}
                  onOpen={() => void actions.openFrame(f.index)}
                  onSelect={() => dispatch({ type: "select/toggle", index: f.index })}
                />
              ))}
            </ul>
          )}
        </main>
        {roll && (
          <aside className="side-panel" aria-label="Roll settings">
            <div className="panel-body">
              <AnalysisAreaPanel
                scope="roll"
                area={analysisArea(roll.roll.shared.geometry)}
                crop={roll.roll.shared.geometry.crop}
                disabled={state.requesting || state.opening}
                hint={
                  awaiting
                    ? undefined
                    : "Changing it analyses the roll again. Frames with their own area keep it."
                }
                onCommit={(area) => void actions.setRollArea(area)}
              >
                {sketchInfo && (
                  <AreaSketch
                    url={state.negativeUrls[sketchInfo.index]}
                    caption={`Frame ${sketchInfo.index + 1}: ${basename(sketchInfo.source)}`}
                    area={analysisArea(roll.roll.shared.geometry)}
                    rotate={roll.roll.shared.geometry.rotate_degrees}
                    flip={roll.roll.shared.geometry.flip_horizontal}
                    disabled={state.requesting || state.opening}
                    onCommit={(area) => void actions.setRollArea(area)}
                  />
                )}
              </AnalysisAreaPanel>
            </div>
            <RollSettingsPanel
              roll={roll.roll}
              disabled={analysing || awaiting}
              onCommit={(r) => void actions.commitRoll(r)}
              onError={(e) => dispatch({ type: "error/set", error: `Could not show the file picker: ${errorText(e)}` })}
            />
          </aside>
        )}
      </div>
    </div>
  );
}
