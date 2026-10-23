// SPDX-License-Identifier: MPL-2.0

//! Integration test on corpus frames: white level pinning.
//!
//! For every RAW frame under `corpus/frames`, the same file decoded twice
//! must give bit-identical data, and frames that share a camera must
//! share black and white levels (they are a roll on one scale). The
//! corpus ships without frames in this repository, so the test passes
//! trivially when the directory holds none; it prints what it checked.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use negg_decode::{DecodeError, Transfer, decode};

/// Extensions rawler decodes that the corpus might contain.
const RAW_EXTENSIONS: &[&str] = &[
    "dng", "cr2", "cr3", "nef", "arw", "raf", "rw2", "orf", "pef",
];

fn corpus_frames() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/frames");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut frames: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| RAW_EXTENSIONS.iter().any(|r| r.eq_ignore_ascii_case(e)))
        })
        .collect();
    frames.sort();
    frames
}

#[test]
fn corpus_frames_pin_their_white_level() {
    let frames = corpus_frames();
    if frames.is_empty() {
        eprintln!("corpus/frames holds no RAW files; nothing to check");
        return;
    }

    let mut levels_by_camera: HashMap<String, ([f32; 3], [f32; 3], PathBuf)> = HashMap::new();
    for frame in &frames {
        let first = match decode(frame) {
            Ok(d) => d,
            Err(DecodeError::Unsupported(why)) => {
                eprintln!("skipping {}: {why}", frame.display());
                continue;
            }
            Err(e) => panic!("{}: {e}", frame.display()),
        };
        let second = decode(frame).unwrap();
        assert_eq!(first.transfer, Transfer::Linear);
        assert_eq!(
            first.image.data(),
            second.image.data(),
            "{} decoded differently twice",
            frame.display()
        );
        assert_eq!(first.white_level, second.white_level);
        assert_eq!(first.black_level, second.black_level);
        for c in 0..3 {
            assert!(
                first.white_level[c] > first.black_level[c],
                "{}: white {:?} must exceed black {:?}",
                frame.display(),
                first.white_level,
                first.black_level
            );
        }

        let camera = first.camera.clone().unwrap_or_else(|| "unknown".into());
        match levels_by_camera.get(&camera) {
            Some((white, black, other)) => {
                // The white level is the camera's calibrated limit and must
                // match exactly. The black level may be read per frame (Canon
                // maker data, or the mean of the masked pixels when rawler has
                // no catalogue value), so it is allowed to drift by up to one
                // code value between frames of one camera.
                assert_eq!(
                    &first.white_level,
                    white,
                    "{} and {} are the same camera but decode on different white levels",
                    frame.display(),
                    other.display()
                );
                for c in 0..3 {
                    assert!(
                        (first.black_level[c] - black[c]).abs() <= 1.0,
                        "{} and {} are the same camera but their black levels differ by more than one code: {:?} vs {:?}",
                        frame.display(),
                        other.display(),
                        first.black_level,
                        black
                    );
                }
            }
            None => {
                levels_by_camera.insert(
                    camera.clone(),
                    (first.white_level, first.black_level, frame.clone()),
                );
            }
        }
        eprintln!(
            "{}: {camera}, {}x{}, white {:?}, black {:?}, warnings {:?}",
            frame.display(),
            first.image.width(),
            first.image.height(),
            first.white_level,
            first.black_level,
            first.warnings
        );
    }
}
