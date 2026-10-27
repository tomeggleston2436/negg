// SPDX-License-Identifier: Apache-2.0
// The app shell: the store, backend event subscriptions, the current screen,
// the export dialog, the error banner, undo and redo on every screen (the
// keyboard, and on macOS the Edit menu), and the close handshake.

import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { closeReady } from "./ipc/commands";
import { CLOSING_EVENT, MENU_EVENT, onAnalysis, onExport, onThumbs } from "./ipc/events";
import { ERROR_EVENT, decodeBackendErrorEvent } from "./ipc/types";
import { historyBlocked } from "./state/reducer";
import { StoreProvider, errorText, useStore } from "./state/store";
import { isTextEntry } from "./focus";
import { RollBrowser } from "./screens/RollBrowser";
import { FrameScreen } from "./screens/FrameScreen";
import { BatchScreen } from "./screens/BatchScreen";
import { ExportDialog } from "./screens/ExportDialog";

/**
 * On macOS the Edit menu's Undo and Redo items carry Cmd+Z and Shift+Cmd+Z
 * and send `negg://menu`; elsewhere the shell handles the keys itself.
 */
const MAC = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent);

function Shell() {
  const { state, dispatch, actions } = useStore();
  const stateRef = useRef(state);
  stateRef.current = state;

  // Undo and redo on every screen, waiting for the analysis as the edits
  // do; never while the export dialog is open or a text field has focus.
  useEffect(() => {
    const step = (redo: boolean) => {
      const st = stateRef.current;
      if (st.exportDialog || historyBlocked(st)) return;
      if (isTextEntry(document.activeElement)) return;
      if (redo) actions.redo();
      else actions.undo();
    };
    const onKey = (e: KeyboardEvent) => {
      if (MAC) return;
      const mod = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();
      if (!mod || e.altKey || (key !== "z" && key !== "y")) return;
      if (isTextEntry(e.target)) return;
      e.preventDefault();
      step(key === "y" || e.shiftKey);
    };
    window.addEventListener("keydown", onKey);
    const menu = listen<{ action?: string }>(MENU_EVENT, (e) => {
      if (e.payload?.action === "undo") step(false);
      else if (e.payload?.action === "redo") step(true);
    });
    // `negg://closing`: the backend is about to flush for a close or quit;
    // the field being typed in commits and every queued edit is sent first.
    const closing = listen(CLOSING_EVENT, () => {
      void actions
        .prepareClose()
        .catch(() => undefined)
        .then(() => closeReady())
        .catch(() => undefined);
    });
    return () => {
      window.removeEventListener("keydown", onKey);
      for (const s of [menu, closing]) void s.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [actions]);

  // The tone presets are not tied to a roll: fetched once at start (and
  // again after a save).
  useEffect(() => {
    void actions.loadPresets();
  }, [actions]);

  useEffect(() => {
    const onError = (e: unknown) => dispatch({ type: "error/set", error: errorText(e) });
    const subs = [
      onAnalysis((event) => {
        dispatch({ type: "analysis/progress", event });
        void actions.refreshRoll();
      }, onError),
      onThumbs((event) => dispatch({ type: "thumbs/stale", frames: event.frames, generation: event.generation }), onError),
      onExport((event) => dispatch({ type: "export/progress", event }), onError),
      // `negg://error`: an error no command returned (a refused sidecar
      // save, a pool that could not be made, a refused close or quit).
      // Shown once on every screen; the views are fetched again so a frame
      // that turned read-only shows it.
      listen(ERROR_EVENT, (e) => {
        try {
          dispatch({ type: "error/set", error: decodeBackendErrorEvent(e.payload).message });
        } catch (err) {
          onError(err);
        }
        void actions.refreshViews();
      }),
    ];
    return () => {
      for (const s of subs) void s.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [dispatch, actions]);

  return (
    <div className="app">
      {state.error && (
        <div className="error-bar" role="alert">
          <span>{state.error}</span>
          <button type="button" onClick={() => dispatch({ type: "error/clear" })} aria-label="Dismiss the error">
            Dismiss
          </button>
        </div>
      )}
      {state.screen === "roll" && <RollBrowser />}
      {state.screen === "frame" && <FrameScreen />}
      {state.screen === "batch" && <BatchScreen />}
      {state.exportDialog && <ExportDialog />}
    </div>
  );
}

export function App() {
  return (
    <StoreProvider>
      <Shell />
    </StoreProvider>
  );
}
