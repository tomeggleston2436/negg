// SPDX-License-Identifier: Apache-2.0

//! The corpus manifest: `corpus/manifest.toml`, its schema and validation.
//!
//! The field names mirror the header comments of `corpus/manifest.toml`
//! exactly. Unknown fields are rejected everywhere, since the manifest is
//! ours and a typo must fail loudly rather than silently drop a frame's
//! grey card or reference.

use std::collections::HashSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The manifest schema version this loader understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Licences a contributor may put on a frame.
const ALLOWED_LICENCES: [&str; 2] = ["Apache-2.0", "CC0-1.0"];

/// The whole manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Schema version; must equal [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// One entry per frame. Absent when the corpus is empty.
    #[serde(default)]
    pub frame: Vec<Frame>,
}

/// One scanned frame.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    /// Unique id, `<roll>-<frame>`.
    pub id: String,
    /// RAW scan, relative to the corpus directory.
    pub file: String,
    /// Roll the frame belongs to.
    pub roll: String,
    /// Film stock.
    pub stock: String,
    /// Scanning camera.
    pub camera: String,
    /// Backlight.
    pub light_source: String,
    /// Whether a grey card is in the frame.
    pub has_grey_card: bool,
    /// The grey card patch; present exactly when `has_grey_card`.
    pub grey_card: Option<Rect>,
    /// Reference renders, possibly none.
    pub references: Vec<Reference>,
    /// Free text about the frame; may be empty.
    #[expect(
        dead_code,
        reason = "part of the manifest schema; eval does not report it yet"
    )]
    pub notes: String,
    /// Who contributed the frame.
    #[expect(
        dead_code,
        reason = "part of the manifest schema; eval does not report it yet"
    )]
    pub contributor: String,
    /// `"Apache-2.0"` or `"CC0-1.0"`.
    pub licence: String,
}

/// A rectangle given as fractions of the image width and height.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    /// Left edge, fraction of width.
    pub x: f32,
    /// Top edge, fraction of height.
    pub y: f32,
    /// Width, fraction of image width.
    pub width: f32,
    /// Height, fraction of image height.
    pub height: f32,
}

/// A reference render of a frame made by an external tool.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    /// The tool that made the render.
    pub tool: Tool,
    /// The render, relative to the corpus directory.
    pub file: String,
    /// The tool's version string.
    #[expect(
        dead_code,
        reason = "part of the manifest schema; eval does not report it yet"
    )]
    pub version: String,
}

/// External tools whose renders serve as references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    /// NegPy.
    Negpy,
    /// darktable's negadoctor module.
    Negadoctor,
}

impl Tool {
    /// The tool's name as written in the manifest and the results JSON.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Negpy => "negpy",
            Tool::Negadoctor => "negadoctor",
        }
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a manifest could not be loaded.
#[derive(Debug)]
pub enum CorpusError {
    /// `manifest.toml` could not be read.
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// `manifest.toml` is not valid TOML for the schema (including an
    /// unknown field).
    Parse(String),
    /// The manifest declares a schema version this loader does not know.
    SchemaVersion(u32),
    /// Two frames share an id.
    DuplicateId(String),
    /// A frame or reference path is not a plain relative path or does
    /// not exist under the corpus directory.
    MissingFile {
        /// Frame id.
        id: String,
        /// The path as written in the manifest.
        file: String,
    },
    /// `grey_card` and `has_grey_card` disagree, or the rect is invalid.
    GreyCard {
        /// Frame id.
        id: String,
        /// What is wrong.
        reason: String,
    },
    /// The licence is not one of the allowed identifiers.
    Licence {
        /// Frame id.
        id: String,
        /// The licence as written.
        licence: String,
    },
}

impl fmt::Display for CorpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CorpusError::Io { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            CorpusError::Parse(msg) => write!(f, "manifest.toml: {msg}"),
            CorpusError::SchemaVersion(v) => write!(
                f,
                "manifest.toml: schema_version {v} is not supported (expected {SCHEMA_VERSION})"
            ),
            CorpusError::DuplicateId(id) => write!(f, "manifest.toml: duplicate frame id {id:?}"),
            CorpusError::MissingFile { id, file } => {
                write!(
                    f,
                    "frame {id:?}: file {file:?} is not a relative path to an existing file in the corpus"
                )
            }
            CorpusError::GreyCard { id, reason } => write!(f, "frame {id:?}: {reason}"),
            CorpusError::Licence { id, licence } => write!(
                f,
                "frame {id:?}: licence {licence:?} is not one of {}",
                ALLOWED_LICENCES.join(", ")
            ),
        }
    }
}

impl std::error::Error for CorpusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CorpusError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Reads and validates `dir/manifest.toml`.
///
/// Validation: the schema version matches, ids are unique, every frame
/// file and reference file is a plain relative path that exists under
/// `dir`, `grey_card` is present exactly when `has_grey_card`, the rect
/// lies within `[0, 1]` with positive size, and the licence is one of the
/// allowed identifiers.
pub fn load(dir: &Path) -> Result<Manifest, CorpusError> {
    let path = dir.join("manifest.toml");
    let text = std::fs::read_to_string(&path).map_err(|source| CorpusError::Io {
        path: path.clone(),
        source,
    })?;
    let manifest = parse(&text)?;
    validate(dir, &manifest)?;
    Ok(manifest)
}

/// Parses manifest text without touching the file system.
fn parse(text: &str) -> Result<Manifest, CorpusError> {
    toml::from_str(text).map_err(|e| CorpusError::Parse(e.to_string()))
}

/// Checks everything `load` promises beyond the TOML schema itself.
fn validate(dir: &Path, manifest: &Manifest) -> Result<(), CorpusError> {
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(CorpusError::SchemaVersion(manifest.schema_version));
    }
    let mut seen = HashSet::new();
    for frame in &manifest.frame {
        if !seen.insert(frame.id.as_str()) {
            return Err(CorpusError::DuplicateId(frame.id.clone()));
        }
        check_file(dir, &frame.id, &frame.file)?;
        for reference in &frame.references {
            check_file(dir, &frame.id, &reference.file)?;
        }
        match (frame.has_grey_card, frame.grey_card) {
            (true, None) => {
                return Err(CorpusError::GreyCard {
                    id: frame.id.clone(),
                    reason: "has_grey_card = true but no grey_card rect".into(),
                });
            }
            (false, Some(_)) => {
                return Err(CorpusError::GreyCard {
                    id: frame.id.clone(),
                    reason: "grey_card given but has_grey_card = false".into(),
                });
            }
            (true, Some(rect)) => check_rect(&frame.id, rect)?,
            (false, None) => {}
        }
        if !ALLOWED_LICENCES.contains(&frame.licence.as_str()) {
            return Err(CorpusError::Licence {
                id: frame.id.clone(),
                licence: frame.licence.clone(),
            });
        }
    }
    Ok(())
}

/// A manifest path must be relative, free of `..`, and name an existing
/// file under the corpus directory.
fn check_file(dir: &Path, id: &str, file: &str) -> Result<(), CorpusError> {
    let rel = Path::new(file);
    let plain = rel.components().all(|c| matches!(c, Component::Normal(_)));
    if !plain || !dir.join(rel).is_file() {
        return Err(CorpusError::MissingFile {
            id: id.to_owned(),
            file: file.to_owned(),
        });
    }
    Ok(())
}

/// The rect must be finite, inside `[0, 1]` and have positive size.
fn check_rect(id: &str, r: Rect) -> Result<(), CorpusError> {
    let finite = [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite());
    let inside = r.x >= 0.0
        && r.y >= 0.0
        && r.width > 0.0
        && r.height > 0.0
        && r.x + r.width <= 1.0
        && r.y + r.height <= 1.0;
    if finite && inside {
        Ok(())
    } else {
        Err(CorpusError::GreyCard {
            id: id.to_owned(),
            reason: format!(
                "grey_card {{ x = {}, y = {}, width = {}, height = {} }} must lie within [0, 1] with positive size",
                r.x, r.y, r.width, r.height
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp directory under `std::env::temp_dir()`, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!(
                "negg-corpus-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn touch(&self, rel: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }

        fn manifest(&self, text: &str) {
            std::fs::write(self.0.join("manifest.toml"), text).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const FRAME: &str = r#"
[[frame]]
id = "r001-03"
file = "frames/r001/r001-03.dng"
roll = "r001"
stock = "Kodak Portra 400"
camera = "Fujifilm X-T4"
light_source = "Cinestill CS-Lite"
has_grey_card = true
grey_card = { x = 0.42, y = 0.70, width = 0.08, height = 0.06 }
references = [
  { tool = "negpy", file = "reference/r001-03.negpy.tif", version = "0.3.1" },
  { tool = "negadoctor", file = "reference/r001-03.negadoctor.tif", version = "darktable 4.8.1" },
]
notes = "Overcast daylight, grey card bottom centre, full border visible."
contributor = "Jane Example"
licence = "CC0-1.0"
"#;

    fn full_corpus(tag: &str) -> TempDir {
        let dir = TempDir::new(tag);
        dir.touch("frames/r001/r001-03.dng");
        dir.touch("reference/r001-03.negpy.tif");
        dir.touch("reference/r001-03.negadoctor.tif");
        dir.manifest(&format!("schema_version = 1\n{FRAME}"));
        dir
    }

    #[test]
    fn empty_manifest_loads() {
        let dir = TempDir::new("empty");
        dir.manifest("schema_version = 1\n");
        let m = load(&dir.0).unwrap();
        assert_eq!(m.schema_version, 1);
        assert!(m.frame.is_empty());
    }

    #[test]
    fn real_corpus_manifest_parses() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
        let m = load(&root).unwrap();
        assert_eq!(m.schema_version, 1);
    }

    #[test]
    fn example_entry_loads() {
        let dir = full_corpus("full");
        let m = load(&dir.0).unwrap();
        assert_eq!(m.frame.len(), 1);
        let f = &m.frame[0];
        assert_eq!(f.id, "r001-03");
        assert_eq!(f.references.len(), 2);
        assert_eq!(f.references[0].tool, Tool::Negpy);
        assert_eq!(f.references[1].tool, Tool::Negadoctor);
        assert_eq!(
            f.grey_card,
            Some(Rect {
                x: 0.42,
                y: 0.70,
                width: 0.08,
                height: 0.06
            })
        );
    }

    #[test]
    fn missing_manifest_is_io() {
        let dir = TempDir::new("nomanifest");
        assert!(matches!(load(&dir.0), Err(CorpusError::Io { .. })));
    }

    #[test]
    fn wrong_schema_version() {
        let dir = TempDir::new("schema");
        dir.manifest("schema_version = 2\n");
        assert!(matches!(load(&dir.0), Err(CorpusError::SchemaVersion(2))));
    }

    #[test]
    fn unknown_field_is_rejected() {
        let dir = full_corpus("unknown");
        dir.manifest(&format!("schema_version = 1\n{FRAME}\ncolour = \"blue\"\n"));
        match load(&dir.0) {
            Err(CorpusError::Parse(msg)) => assert!(msg.contains("colour"), "{msg}"),
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let dir = full_corpus("dup");
        dir.manifest(&format!("schema_version = 1\n{FRAME}\n{FRAME}"));
        match load(&dir.0) {
            Err(CorpusError::DuplicateId(id)) => assert_eq!(id, "r001-03"),
            other => panic!("expected DuplicateId, got {other:?}"),
        }
    }

    #[test]
    fn missing_frame_file_is_rejected() {
        let dir = full_corpus("missingframe");
        std::fs::remove_file(dir.0.join("frames/r001/r001-03.dng")).unwrap();
        match load(&dir.0) {
            Err(CorpusError::MissingFile { id, file }) => {
                assert_eq!(id, "r001-03");
                assert_eq!(file, "frames/r001/r001-03.dng");
            }
            other => panic!("expected MissingFile, got {other:?}"),
        }
    }

    #[test]
    fn missing_reference_file_is_rejected() {
        let dir = full_corpus("missingref");
        std::fs::remove_file(dir.0.join("reference/r001-03.negadoctor.tif")).unwrap();
        match load(&dir.0) {
            Err(CorpusError::MissingFile { file, .. }) => {
                assert_eq!(file, "reference/r001-03.negadoctor.tif");
            }
            other => panic!("expected MissingFile, got {other:?}"),
        }
    }

    #[test]
    fn parent_traversal_is_rejected() {
        let dir = full_corpus("traversal");
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace("frames/r001/r001-03.dng", "../manifest.toml")
        ));
        assert!(matches!(load(&dir.0), Err(CorpusError::MissingFile { .. })));
    }

    #[test]
    fn grey_card_flag_and_rect_must_agree() {
        let dir = full_corpus("greycard");
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace("has_grey_card = true", "has_grey_card = false")
        ));
        assert!(matches!(load(&dir.0), Err(CorpusError::GreyCard { .. })));

        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace(
                "grey_card = { x = 0.42, y = 0.70, width = 0.08, height = 0.06 }\n",
                ""
            )
        ));
        assert!(matches!(load(&dir.0), Err(CorpusError::GreyCard { .. })));
    }

    #[test]
    fn rect_must_be_inside_unit_square_with_positive_size() {
        let dir = full_corpus("rect");
        for bad in [
            "{ x = 0.95, y = 0.1, width = 0.1, height = 0.1 }",
            "{ x = 0.1, y = 0.1, width = 0.0, height = 0.1 }",
            "{ x = -0.1, y = 0.1, width = 0.1, height = 0.1 }",
            "{ x = 0.1, y = 0.1, width = 0.1, height = -0.1 }",
        ] {
            dir.manifest(&format!(
                "schema_version = 1\n{}",
                FRAME.replace("{ x = 0.42, y = 0.70, width = 0.08, height = 0.06 }", bad)
            ));
            assert!(
                matches!(load(&dir.0), Err(CorpusError::GreyCard { .. })),
                "{bad}"
            );
        }
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace(
                "{ x = 0.42, y = 0.70, width = 0.08, height = 0.06 }",
                "{ x = 0.0, y = 0.0, width = 1.0, height = 1.0 }"
            )
        ));
        assert!(load(&dir.0).is_ok());
    }

    #[test]
    fn licence_must_be_allowed() {
        let dir = full_corpus("licence");
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace("licence = \"CC0-1.0\"", "licence = \"GPL-3.0\"")
        ));
        match load(&dir.0) {
            Err(CorpusError::Licence { licence, .. }) => assert_eq!(licence, "GPL-3.0"),
            other => panic!("expected Licence, got {other:?}"),
        }
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace("licence = \"CC0-1.0\"", "licence = \"Apache-2.0\"")
        ));
        assert!(load(&dir.0).is_ok());
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let dir = full_corpus("tool");
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace("tool = \"negpy\"", "tool = \"lightroom\"")
        ));
        assert!(matches!(load(&dir.0), Err(CorpusError::Parse(_))));
    }

    #[test]
    fn tool_names_round_trip() {
        assert_eq!(Tool::Negpy.to_string(), "negpy");
        assert_eq!(Tool::Negadoctor.name(), "negadoctor");
        assert_eq!(
            serde_json::to_string(&Tool::Negadoctor).unwrap(),
            "\"negadoctor\""
        );
    }
}
