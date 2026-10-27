// SPDX-License-Identifier: Apache-2.0

//! The frontend's IPC fixtures (app/src/ipc/__fixtures__/*.json) are the
//! real serde output of sample values ("### Tauri backend (P3-T)"): this
//! test fails when they differ, and `NEGG_BLESS=1` rewrites them.

use std::path::PathBuf;

use negg_app_lib::state::AppRollView;
use negg_session::{
    ExportFormat, ExportRequest, ExportScope, FrameInfo, FrameStatus, FrameView, HistoryView,
    NegativeView, RollView, TILE_SIZE,
};
use negg_types::estimates::{CastCorrection, CastSource, DminSource, Estimates, GammaSource};
use negg_types::roll::{PoolFrame, PoolRole, resolve};
use negg_types::settings::ChannelEstimate;
use negg_types::{CropRect, FrameOverrides, Geometry, RollSettings, Settings};
use serde::Serialize;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/ipc/__fixtures__")
}

fn history() -> HistoryView {
    HistoryView {
        can_undo: true,
        can_redo: false,
        len: 2,
        cursor: 2,
        pool_pending: false,
    }
}

fn settings() -> Settings {
    Settings::default()
}

fn roll_settings() -> RollSettings {
    RollSettings::default()
}

fn frame_overrides() -> FrameOverrides {
    let mut o = FrameOverrides::default();
    o.film.film_base = Some(ChannelEstimate::Fixed {
        value: [0.21, 0.55, 0.78],
    });
    o.print.grade = Some(120.0);
    o.print.exposure = Some(0.25);
    o.print.cmy = Some([0.0, 0.01, -0.02]);
    o.print.black_point_compensation = Some(true);
    o.print.saturation = Some(1.15);
    // Pinned "no preset" (Ruling 8).
    o.print.preset = Some(String::new());
    o.geometry = Some(Geometry {
        crop: Some(CropRect {
            x: 0.05,
            y: 0.04,
            width: 0.9,
            height: 0.92,
        }),
        analysis_area: Some(CropRect {
            x: 0.02,
            y: 0.03,
            width: 0.96,
            height: 0.94,
        }),
        analysis_buffer: 0.05,
        rotate_degrees: 90,
        flip_horizontal: true,
    });
    o
}

fn cast() -> CastCorrection {
    CastCorrection {
        slope_r: 1.05,
        offset_r: -0.01,
        slope_b: 0.97,
        offset_b: 0.02,
        cells_used: 123,
    }
}

fn roll_view() -> RollView {
    let frame = |index: u32, status, message: Option<&str>| FrameInfo {
        index,
        source: format!("frame-{:02}.tif", index + 1),
        status,
        message: message.map(str::to_owned),
        needs_analysis_area: false,
    };
    RollView {
        dir: "/rolls/roll-01".to_owned(),
        frames: vec![
            FrameInfo {
                needs_analysis_area: true,
                ..frame(0, FrameStatus::Ready, None)
            },
            frame(
                1,
                FrameStatus::Failed,
                Some("decode: could not decode frame-02.tif"),
            ),
            frame(
                2,
                FrameStatus::ReadOnly,
                Some("sidecar frame-03.tif.negg.json could not be read"),
            ),
            frame(3, FrameStatus::Pending, None),
        ],
        roll: roll_settings(),
        analysed: false,
        generation: 7,
        history: history(),
        pool_pending: false,
        pool_warning: Some(negg_session::BORDER_LOST_WARNING.to_owned()),
    }
}

fn frame_view() -> FrameView {
    let overrides = frame_overrides();
    FrameView {
        index: 0,
        source: "frame-01.tif".to_owned(),
        width: 6000,
        height: 4000,
        levels: 5,
        tile_size: TILE_SIZE,
        settings: resolve(&roll_settings(), &overrides),
        overrides,
        estimates: Some(Estimates {
            dmin: [0.21, 0.55, 0.78],
            dmin_source: DminSource::Fixed,
            dmax: [1.9, 2.2, 2.4],
            gamma: [1.6, 1.6, 1.6],
            gamma_source: GammaSource::Roll,
            neutral_offset: [0.0, 0.0, 0.0],
            anchor: -0.42,
            x0: 1.25,
            cast: cast(),
            cast_source: CastSource::Roll,
        }),
        pool: Some(PoolFrame {
            source: "frame-01.tif".to_owned(),
            dmin: [0.21, 0.55, 0.78],
            dmin_source: DminSource::Border,
            dmax: [1.9, 2.2, 2.4],
            gamma: [1.6, 1.6, 1.6],
            cast: cast(),
            dmin_role: PoolRole::Inlier,
            gamma_role: PoolRole::Inlier,
            cast_role: PoolRole::Unused,
            needs_analysis_area: false,
        }),
        generation: 7,
        read_only: false,
        history: history(),
        pool_pending: false,
        needs_analysis_area: true,
    }
}

/// The roll view as the app's commands send it.
fn app_roll_view() -> AppRollView {
    AppRollView {
        view: roll_view(),
        awaiting_analysis: true,
    }
}

fn negative_view() -> NegativeView {
    NegativeView {
        index: 2,
        source: "frame-03.tif".to_owned(),
        width: 6000,
        height: 4000,
        levels: 5,
        tile_size: TILE_SIZE,
    }
}

fn export_request() -> ExportRequest {
    ExportRequest {
        scope: ExportScope::Frames { frames: vec![0, 2] },
        format: ExportFormat::Jpeg,
        out_dir: Some("/exports/roll-01".to_owned()),
        web: negg_session::WebOutput::None,
    }
}

fn preset_list() -> negg_session::PresetList {
    let mut list = negg_session::list_presets(None);
    list.presets.truncate(2);
    list.builtins = 2;
    list.warnings = vec!["broken.preset.negg.toml: unreadable".to_owned()];
    list
}

/// Compares (or with `NEGG_BLESS=1` writes) one fixture.
fn check<T: Serialize>(name: &str, value: &T) -> Option<String> {
    let path = fixtures_dir().join(name);
    let json = serde_json::to_string_pretty(value).unwrap() + "\n";
    if std::env::var_os("NEGG_BLESS").is_some_and(|v| v == "1") {
        std::fs::write(&path, &json).unwrap();
        return None;
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    (on_disk != json).then(|| {
        format!(
            "{} differs from the serde output; run with NEGG_BLESS=1 to rewrite it:\n{json}",
            path.display()
        )
    })
}

#[test]
fn fixtures_are_the_serde_output() {
    let stale: Vec<String> = [
        check("settings.json", &settings()),
        check("frame_overrides.json", &frame_overrides()),
        check("roll_view.json", &app_roll_view()),
        check("negative_view.json", &negative_view()),
        check("frame_view.json", &frame_view()),
        check("export_request.json", &export_request()),
        check("preset_list.json", &preset_list()),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert!(stale.is_empty(), "{}", stale.join("\n"));
}

#[test]
fn fixtures_parse_back() {
    let read = |name: &str| std::fs::read_to_string(fixtures_dir().join(name)).unwrap();
    assert_eq!(
        serde_json::from_str::<RollView>(&read("roll_view.json")).unwrap(),
        roll_view()
    );
    assert_eq!(
        serde_json::from_str::<FrameView>(&read("frame_view.json")).unwrap(),
        frame_view()
    );
    assert_eq!(
        serde_json::from_str::<ExportRequest>(&read("export_request.json")).unwrap(),
        export_request()
    );
    assert_eq!(
        serde_json::from_str::<NegativeView>(&read("negative_view.json")).unwrap(),
        negative_view()
    );
}
