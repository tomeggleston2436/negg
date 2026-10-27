// SPDX-License-Identifier: MPL-2.0

//! Every committed `profiles/**/*.negg.toml` must load and validate.
//! Zero files passes: no measured profile is committed yet.

use std::fs;
use std::path::{Path, PathBuf};

use negg_io::{read_film_profile, read_rig_profile};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".negg.toml"))
        {
            out.push(path);
        }
    }
}

#[test]
fn every_committed_profile_loads_and_validates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    assert!(root.is_dir(), "{} must exist", root.display());
    let mut files = Vec::new();
    collect(&root, &mut files);
    for path in files {
        let name = path.file_name().unwrap().to_str().unwrap();
        if name.ends_with(".rig.negg.toml") {
            read_rig_profile(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        } else if name.ends_with(".film.negg.toml") {
            read_film_profile(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        } else {
            panic!(
                "{} must be named <name>.rig.negg.toml or <name>.film.negg.toml",
                path.display()
            );
        }
    }
}

#[test]
fn layout_directories_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    assert!(root.join("rig").is_dir());
    assert!(root.join("film").is_dir());
}
