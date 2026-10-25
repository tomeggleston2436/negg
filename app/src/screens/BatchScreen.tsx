// SPDX-License-Identifier: Apache-2.0
// The batch screen: follows an export with per-frame progress from
// `negg://export` and shows the final report.

import { useStore } from "../state/store";
import { basename } from "../components/RollSettingsPanel";

export function BatchScreen() {
  const { state, dispatch } = useStore();
  const p = state.exportProgress;

  return (
    <div className="batch-screen">
      <header className="toolbar">
        <button type="button" onClick={() => dispatch({ type: "screen/show", screen: "roll" })}>
          Back to roll
        </button>
        {state.current !== null && (
          <button type="button" onClick={() => dispatch({ type: "screen/show", screen: "frame" })}>
            Back to frame
          </button>
        )}
        <span className="toolbar-title">Export</span>
      </header>
      <main className="batch-body">
        {!p && <p className="muted">No export has run in this session.</p>}
        {p && (
          <>
            <div className="progress-line" role="status" aria-live="polite">
              <progress max={Math.max(1, p.total)} value={p.done} />
              <span>
                {p.running
                  ? `Exporting ${p.done} of ${p.total}`
                  : p.error
                    ? "Export stopped"
                    : `Finished: ${p.report?.converted ?? 0} converted, ${p.report?.failed ?? 0} failed`}
              </span>
            </div>
            <p className="muted">
              {p.request.format === "tiff16" ? "TIFF, 16-bit" : "JPEG"} to{" "}
              {p.request.out_dir ?? "the folder beside each source"}
            </p>
            {p.error && (
              <p className="banner warn" role="alert">
                {p.error}
              </p>
            )}
            <ol className="batch-list">
              {p.items.map((it, i) => (
                <li key={i} className={it.ok ? "ok" : "failed"}>
                  <span className={`badge ${it.ok ? "badge-ready" : "badge-failed"}`}>{it.ok ? "Done" : "Failed"}</span>
                  <span title={it.input}>{basename(it.input)}</span>
                  {it.message && <span className="muted"> {it.message}</span>}
                </li>
              ))}
            </ol>
            {p.report && p.report.errors.length > 0 && (
              <section aria-labelledby="export-errors">
                <h2 id="export-errors">Errors</h2>
                <ul>
                  {p.report.errors.map((e, i) => (
                    <li key={i}>{e}</li>
                  ))}
                </ul>
              </section>
            )}
            {p.report && p.report.outputs.length > 0 && (
              <section aria-labelledby="export-outputs">
                <h2 id="export-outputs">Written</h2>
                <ul className="paths">
                  {p.report.outputs.map((o) => (
                    <li key={o}>{o}</li>
                  ))}
                </ul>
              </section>
            )}
          </>
        )}
      </main>
    </div>
  );
}
