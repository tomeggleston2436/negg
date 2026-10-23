// SPDX-License-Identifier: MPL-2.0

//! The worked examples in docs/PROFILES.md must match the serde schema.
//! Each is rejected as written (its `source` carries `TODO(measure)`,
//! ground rule 6) and loads once the placeholders are made valid.

use std::fs;
use std::path::{Path, PathBuf};

use negg_io::{IoError, read_film_profile, read_rig_profile};

type Reader = fn(&Path) -> Result<(), IoError>;

fn doc_blocks() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/PROFILES.md");
    let text = fs::read_to_string(&path).unwrap();
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        match (&mut current, line.trim_end()) {
            (None, "```toml") => current = Some(String::new()),
            (Some(_), "```") => blocks.push(current.take().unwrap()),
            (Some(b), l) => {
                b.push_str(l);
                b.push('\n');
            }
            _ => {}
        }
    }
    blocks
}

fn temp(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("negg-io-doc-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path
}

/// Replaces deliberate placeholders with valid values.
fn fix(block: &str) -> String {
    let mut out = String::new();
    for line in block.lines() {
        let code = line.split('#').next().unwrap().trim_end();
        let fixed = if code.contains("TODO(measure)") {
            let key = code.split('=').next().unwrap().trim();
            format!("{key} = \"synthetic test source\"")
        } else {
            match code.split('=').next().unwrap().trim() {
                "width" | "height" if code.contains("= 0") => code.replace("= 0", "= 100"),
                "iso" | "n_patches" if code.contains("= 0") => code.replace("= 0", "= 4"),
                _ => line.to_owned(),
            }
        };
        out.push_str(&fixed);
        out.push('\n');
    }
    out
}

#[test]
fn doc_examples_match_the_schema() {
    let blocks = doc_blocks();
    let mut seen = (0, 0);
    for (i, block) in blocks.iter().enumerate() {
        let rig = block.contains("[flat_field]");
        let film = block.contains("[stock]");
        assert!(rig || film, "block {i} is neither a rig nor a film example");
        let (kind, read): (&str, Reader) = if rig {
            seen.0 += 1;
            ("rig", |p| read_rig_profile(p).map(|_| ()))
        } else {
            seen.1 += 1;
            ("film", |p| read_film_profile(p).map(|_| ()))
        };
        let name = format!("ex{i}.{kind}.negg.toml");
        match read(&temp(&name, block)) {
            Err(IoError::Profile(m)) => {
                assert!(
                    m.contains("TODO(measure)"),
                    "{kind} example as written: {m}"
                );
            }
            other => panic!("{kind} example as written must be rejected, got {other:?}"),
        }
        let fixed = fix(block);
        read(&temp(&format!("fixed{name}"), &fixed))
            .unwrap_or_else(|e| panic!("{kind} example after fixing placeholders: {e}"));
    }
    assert_eq!(seen, (1, 1), "expected one rig and one film example");
}
