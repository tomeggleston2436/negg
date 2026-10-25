// SPDX-License-Identifier: Apache-2.0

//! The views a [`crate::Session`] answers with, and the export request and
//! report. They serialise with serde in snake case, like the sidecars, and
//! are the JSON shared by the desktop app's commands and the C ABI.

use negg_types::{Estimates, FrameOverrides, PoolFrame, RollSettings, Settings};
use serde::{Deserialize, Serialize};

/// The open roll: its directory, its frames, its settings, whether pass 1
/// has finished, the session generation and the edit history.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RollView {
    /// The roll directory.
    pub dir: String,
    /// Every batch input of the directory, in name order.
    pub frames: Vec<FrameInfo>,
    /// The roll settings as they are now (committed).
    pub roll: RollSettings,
    /// True once pass 1 and pooling have finished; edits and exports wait
    /// until then (Ruling 81).
    pub analysed: bool,
    /// The session generation (stamped into tiles and scopes).
    pub generation: u32,
    /// The edit history.
    pub history: HistoryView,
    /// An edit changed what the roll pool sees and the pool has not been
    /// made again yet (Ruling 124): the host calls `analyse_next` until it
    /// returns Done, then fetches its views again. Absent in older JSON,
    /// read as false.
    #[serde(default)]
    pub pool_pending: bool,
    /// Set when the last analysis area or crop change took the roll's
    /// film base (Dmin) pool off the clear film edge (Ruling 138): the
    /// text to show, which says to cover the whole film including its
    /// clear edge (rebate) and leave out only the holder. Omitted when
    /// unset; absent in older JSON, read as unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_warning: Option<String>,
}

/// Where a frame is in the session.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrameStatus {
    /// Not analysed yet.
    Pending,
    /// Analysed; it can be viewed, edited and exported.
    Ready,
    /// It could not be analysed or can never be converted; the message
    /// says why.
    Failed,
    /// Its sidecar could not be read or changed on disk; it can be viewed
    /// but not edited, and its sidecar is never written (Ruling 80).
    ReadOnly,
}

/// One frame of the roll as the roll browser shows it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FrameInfo {
    /// The frame index.
    pub index: u32,
    /// The source file name.
    pub source: String,
    /// Its status.
    pub status: FrameStatus,
    /// Why it failed or is read-only; for a ready frame, the decoder's
    /// notes, if any.
    pub message: Option<String>,
    /// The frame's Dmax is pinned at the density cap (Ruling 133): the film
    /// holder or another opaque area is in its analysed area, it is left
    /// out of the gamma and cast pools, and the app shows "needs analysis
    /// area". False before pass 1 reaches it. It reflects the frame's last
    /// completed analysis: while `pool_pending` is true after an analysis
    /// area or crop edit it may still show the old area. Absent in older
    /// JSON, read as false.
    #[serde(default)]
    pub needs_analysis_area: bool,
}

/// One frame as the frame screen shows it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FrameView {
    /// The frame index.
    pub index: u32,
    /// The source file name.
    pub source: String,
    /// Width of the source in pixels, in its decoded orientation.
    pub width: u32,
    /// Height of the source in pixels, in its decoded orientation.
    pub height: u32,
    /// Pyramid levels including level 0 (`L + 1`, Ruling 73).
    pub levels: u32,
    /// Side of a preview tile in pixels.
    pub tile_size: u32,
    /// The frame's committed overrides.
    pub overrides: FrameOverrides,
    /// The settings the frame resolves to (roll settings with its
    /// overrides).
    pub settings: Settings,
    /// The estimates the frame renders and exports with, when it could be
    /// planned.
    pub estimates: Option<Estimates>,
    /// The frame's record in the roll pool, when it is pooled.
    pub pool: Option<PoolFrame>,
    /// The session generation.
    pub generation: u32,
    /// True when the frame cannot be edited (its sidecar is never written).
    pub read_only: bool,
    /// The edit history.
    pub history: HistoryView,
    /// A re-pool is pending (Rulings 124 and 126), as in [`RollView`]: the
    /// estimates shown are on the previous pool until the host has run
    /// `analyse_next` to Done. Absent in older JSON, read as false.
    #[serde(default)]
    pub pool_pending: bool,
    /// As [`FrameInfo::needs_analysis_area`] (Ruling 133). Absent in older
    /// JSON, read as false.
    #[serde(default)]
    pub needs_analysis_area: bool,
}

/// A frame's decoded negative as the pre-analysis step shows it
/// (Ruling 134): its size and pyramid, for
/// [`crate::Session::negative_tile`]. Available before pass 1.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NegativeView {
    /// The frame index.
    pub index: u32,
    /// The source file name.
    pub source: String,
    /// Width of the source in pixels, in its decoded orientation.
    pub width: u32,
    /// Height of the source in pixels, in its decoded orientation.
    pub height: u32,
    /// Pyramid levels including level 0 (`L + 1`, Ruling 73).
    pub levels: u32,
    /// Side of a tile in pixels.
    pub tile_size: u32,
}

/// The edit history (Ruling 79): one linear list with a cursor.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryView {
    /// True when there is an edit to undo.
    pub can_undo: bool,
    /// True when there is an undone edit to redo.
    pub can_redo: bool,
    /// Number of edits recorded.
    pub len: u32,
    /// Number of edits applied (the rest are the redo tail).
    pub cursor: u32,
    /// A re-pool is pending (Rulings 124 and 126), as in [`RollView`], so
    /// that undo and redo tell the host to run `analyse_next` to Done.
    /// Absent in older JSON, read as false.
    #[serde(default)]
    pub pool_pending: bool,
}

/// What to export (Ruling 82).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportScope {
    /// One frame, as `negg convert <frame>`.
    Frame {
        /// The frame index.
        frame: u32,
    },
    /// Several frames, each as `negg convert <frame>`.
    Frames {
        /// The frame indices, exported in this order.
        frames: Vec<u32>,
    },
    /// The whole roll, as `negg batch <dir>`.
    Roll,
}

/// The output file format of an export.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// 16-bit TIFF (`--format tiff16`).
    Tiff16,
    /// 8-bit JPEG (`--format jpeg`).
    Jpeg,
}

/// An export: what, in which format, and where.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportRequest {
    /// What to export.
    pub scope: ExportScope,
    /// The output format.
    pub format: ExportFormat,
    /// The output directory (`--out`); `None` writes beside the sources, as
    /// the CLI does.
    pub out_dir: Option<String>,
}

/// What an export did.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ExportReport {
    /// Frames converted.
    pub converted: u32,
    /// Frames that failed.
    pub failed: u32,
    /// The output images written, in the order they were written.
    pub outputs: Vec<String>,
    /// One line per failed frame: its path and why.
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_requests_use_snake_case_tags() {
        let req = ExportRequest {
            scope: ExportScope::Frames { frames: vec![1, 2] },
            format: ExportFormat::Tiff16,
            out_dir: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(
            json,
            r#"{"scope":{"kind":"frames","frames":[1,2]},"format":"tiff16","out_dir":null}"#
        );
        let back: ExportRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
        let roll: ExportScope = serde_json::from_str(r#"{"kind":"roll"}"#).unwrap();
        assert_eq!(roll, ExportScope::Roll);
        assert_eq!(
            serde_json::to_string(&FrameStatus::ReadOnly).unwrap(),
            r#""read_only""#
        );
    }
}
