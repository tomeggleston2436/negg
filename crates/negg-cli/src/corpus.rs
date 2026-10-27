// SPDX-License-Identifier: Apache-2.0

//! The corpus manifest: `corpus/manifest.toml`, its schema and validation.
//!
//! The field names mirror the header comments of `corpus/manifest.toml`
//! exactly. Unknown fields are rejected everywhere, since the manifest is
//! ours and a typo must fail loudly rather than silently drop a frame's
//! grey card or reference.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The manifest schema version this loader understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Licences a contributor may put on a frame.
const ALLOWED_LICENCES: [&str; 3] = ["Apache-2.0", "CC0-1.0", PRIVATE_LICENCE];

/// The licence of a frame that is not licensed to anyone: a private corpus
/// kept outside this repository and used only for local evals. The committed
/// `corpus/manifest.toml` must never carry it (checked by a test).
pub const PRIVATE_LICENCE: &str = "LicenseRef-Private";

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
    /// Film stock. Frames of one roll share it.
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
    pub notes: String,
    /// Who contributed the frame.
    pub contributor: String,
    /// `"Apache-2.0"` or `"CC0-1.0"`.
    pub licence: String,
    /// The physical film roll the frame was cut from, when it is scanned
    /// more than once (for example under several light sources, each scan
    /// its own `roll`). Groups frames for the Gate 2 three-lights metric.
    /// Frames of one film roll share `stock`.
    #[serde(default)]
    pub film_roll: Option<String>,
    /// The rig profile (`*.rig.negg.toml`) the roll is rendered with,
    /// relative to the corpus directory. Frames of one roll share it.
    #[serde(default)]
    pub rig: Option<String>,
    /// The film profile (`*.film.negg.toml`) the roll is rendered with,
    /// relative to the corpus directory. Frames of one roll share it.
    #[serde(default)]
    pub film_profile: Option<String>,
    /// The frame's geometry (crop, analysis buffer, rotation and flip), as
    /// `Settings.geometry` takes it. `negg eval` renders the frame with it
    /// as a frame override in roll mode (the whole geometry), so
    /// the film holder can be cropped away per frame. Absent means the
    /// roll's default geometry, exactly as before the field existed.
    #[serde(default)]
    pub geometry: Option<FrameGeometry>,
}

/// A frame's geometry as written in the manifest: the fields and defaults
/// of [`negg_types::Geometry`], with unknown fields rejected like the rest
/// of the manifest. Validated by [`negg_types::Geometry::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameGeometry {
    /// The crop, normalised to the source frame in its decoded orientation;
    /// absent is the whole frame.
    #[serde(default)]
    pub crop: Option<Crop>,
    /// Fraction of the analysis rectangle's short side trimmed from every
    /// side of it for analysis only, in `[0, 0.2]`; defaults to
    /// [`negg_types::ANALYSIS_BUFFER_DEFAULT`]. The manifest has no
    /// analysis area yet, so the analysis rectangle is the crop, and the
    /// buffer is ignored without one.
    #[serde(default = "default_analysis_buffer")]
    pub analysis_buffer: f32,
    /// Clockwise rotation in degrees: 0, 90, 180 or 270.
    #[serde(default)]
    pub rotate_degrees: u16,
    /// Mirror the rotated image left to right.
    #[serde(default)]
    pub flip_horizontal: bool,
}

/// A crop rectangle as written in the manifest: the fields of
/// [`negg_types::CropRect`], every one required.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    /// Left edge, fraction of the frame width.
    pub x: f32,
    /// Top edge, fraction of the frame height.
    pub y: f32,
    /// Width, fraction of the frame width; at least 0.01.
    pub width: f32,
    /// Height, fraction of the frame height; at least 0.01.
    pub height: f32,
}

fn default_analysis_buffer() -> f32 {
    negg_types::ANALYSIS_BUFFER_DEFAULT
}

impl FrameGeometry {
    /// The engine's [`negg_types::Geometry`] with the same fields.
    pub fn to_geometry(&self) -> negg_types::Geometry {
        negg_types::Geometry {
            crop: self.crop.map(|c| negg_types::CropRect {
                x: c.x,
                y: c.y,
                width: c.width,
                height: c.height,
            }),
            analysis_area: None,
            analysis_buffer: self.analysis_buffer,
            rotate_degrees: self.rotate_degrees,
            flip_horizontal: self.flip_horizontal,
        }
    }
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
    /// Negative Lab Pro: an 8-bit sRGB JPEG (or TIFF) export, cropped by
    /// hand and possibly mirrored or rotated, so it is always registered
    /// to the render before scoring (docs/ADR/0028).
    Nlp,
}

impl Tool {
    /// The tool's name as written in the manifest and the results JSON.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Negpy => "negpy",
            Tool::Negadoctor => "negadoctor",
            Tool::Nlp => "nlp",
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
    /// A frame or reference path is not a plain relative path (it is
    /// absolute, or has a `.` or `..` component). Whether the file exists
    /// is not checked here: the corpus is local-only and `negg eval` skips
    /// entries whose files are absent.
    BadPath {
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
    /// A `rig` or `film_profile` path is empty or absolute.
    BadProfilePath {
        /// Frame id.
        id: String,
        /// The path as written in the manifest.
        file: String,
    },
    /// Two frames that must agree on a field do not: frames of one `roll`
    /// on `stock`, `rig` or `film_profile`, or frames of one `film_roll` on
    /// `stock`.
    Inconsistent {
        /// The frame that disagrees with an earlier one.
        id: String,
        /// `roll` or `film_roll`.
        group: &'static str,
        /// The roll or film roll name.
        name: String,
        /// The field that differs.
        field: &'static str,
    },
    /// A frame's `geometry` fails validation.
    Geometry {
        /// Frame id.
        id: String,
        /// What is wrong.
        reason: String,
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
            CorpusError::BadPath { id, file } => {
                write!(
                    f,
                    "frame {id:?}: file {file:?} must be a relative path inside the corpus with no `.` or `..` components"
                )
            }
            CorpusError::GreyCard { id, reason } => write!(f, "frame {id:?}: {reason}"),
            CorpusError::Geometry { id, reason } => write!(f, "frame {id:?}: geometry: {reason}"),
            CorpusError::Licence { id, licence } => write!(
                f,
                "frame {id:?}: licence {licence:?} is not one of {}",
                ALLOWED_LICENCES.join(", ")
            ),
            CorpusError::BadProfilePath { id, file } => write!(
                f,
                "frame {id:?}: profile path {file:?} must be a non-empty path relative to the corpus directory"
            ),
            CorpusError::Inconsistent {
                id,
                group,
                name,
                field,
            } => write!(
                f,
                "frame {id:?}: {field} differs from an earlier frame of {group} {name:?}; frames of one {group} must share it"
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
/// file and reference file is a plain relative path (no absolute, `.` or
/// `..` components), `grey_card` is present exactly when `has_grey_card`,
/// the rect lies within `[0, 1]` with positive size, and the licence is
/// one of the allowed identifiers, a `geometry` passes the checks of
/// `Settings.geometry`, `rig` and `film_profile` are
/// non-empty relative paths shared by every frame of a roll, every frame
/// of a roll has the same stock, and frames of one `film_roll` share their
/// stock. Whether the files exist is not
/// checked: the corpus is local-only (docs/ADR/0005), so a manifest entry
/// whose file is absent on this machine is accepted here and skipped by
/// `negg eval`.
pub fn load(dir: &Path) -> Result<Manifest, CorpusError> {
    let path = dir.join("manifest.toml");
    let text = std::fs::read_to_string(&path).map_err(|source| CorpusError::Io {
        path: path.clone(),
        source,
    })?;
    let manifest = parse(&text)?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Parses manifest text without touching the file system.
fn parse(text: &str) -> Result<Manifest, CorpusError> {
    toml::from_str(text).map_err(|e| CorpusError::Parse(e.to_string()))
}

/// Checks everything `load` promises beyond the TOML schema itself.
fn validate(manifest: &Manifest) -> Result<(), CorpusError> {
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(CorpusError::SchemaVersion(manifest.schema_version));
    }
    let mut seen = HashSet::new();
    // First frame seen of each roll and of each film roll.
    let mut rolls: HashMap<&str, &Frame> = HashMap::new();
    let mut film_rolls: HashMap<&str, &Frame> = HashMap::new();
    for frame in &manifest.frame {
        if !seen.insert(frame.id.as_str()) {
            return Err(CorpusError::DuplicateId(frame.id.clone()));
        }
        check_path(&frame.id, &frame.file)?;
        for reference in &frame.references {
            check_path(&frame.id, &reference.file)?;
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
        if let Some(geometry) = &frame.geometry {
            check_geometry(&frame.id, geometry)?;
        }
        if !ALLOWED_LICENCES.contains(&frame.licence.as_str()) {
            return Err(CorpusError::Licence {
                id: frame.id.clone(),
                licence: frame.licence.clone(),
            });
        }
        for path in [&frame.rig, &frame.film_profile].into_iter().flatten() {
            check_profile_path(&frame.id, path)?;
        }
        let inconsistent = |group, name: &str, field| CorpusError::Inconsistent {
            id: frame.id.clone(),
            group,
            name: name.to_owned(),
            field,
        };
        let first = *rolls.entry(frame.roll.as_str()).or_insert(frame);
        // The eval takes a roll's stock from its first frame.
        if first.stock != frame.stock {
            return Err(inconsistent("roll", &frame.roll, "stock"));
        }
        if first.rig != frame.rig {
            return Err(inconsistent("roll", &frame.roll, "rig"));
        }
        if first.film_profile != frame.film_profile {
            return Err(inconsistent("roll", &frame.roll, "film_profile"));
        }
        if let Some(film_roll) = &frame.film_roll {
            let first = *film_rolls.entry(film_roll.as_str()).or_insert(frame);
            if first.stock != frame.stock {
                return Err(inconsistent("film_roll", film_roll, "stock"));
            }
        }
    }
    Ok(())
}

/// A `rig` or `film_profile` path must be non-empty and relative (it is
/// resolved against the corpus directory). Unlike frame paths it may
/// climb out of the corpus with `..`, so a roll can use a profile
/// committed under the repository's `profiles/`.
fn check_profile_path(id: &str, file: &str) -> Result<(), CorpusError> {
    let path = Path::new(file);
    if file.is_empty() || path.is_absolute() || path.has_root() {
        return Err(CorpusError::BadProfilePath {
            id: id.to_owned(),
            file: file.to_owned(),
        });
    }
    Ok(())
}

/// A manifest path must be relative and made only of plain components,
/// so that joining it to the corpus directory cannot escape it. The file
/// need not exist.
fn check_path(id: &str, file: &str) -> Result<(), CorpusError> {
    let mut components = Path::new(file).components().peekable();
    let plain =
        components.peek().is_some() && components.all(|c| matches!(c, Component::Normal(_)));
    if !plain {
        return Err(CorpusError::BadPath {
            id: id.to_owned(),
            file: file.to_owned(),
        });
    }
    Ok(())
}

/// The geometry must pass [`negg_types::Geometry::validate`], the rule
/// `Settings.geometry` is held to.
fn check_geometry(id: &str, geometry: &FrameGeometry) -> Result<(), CorpusError> {
    geometry
        .to_geometry()
        .validate()
        .map_err(|e| CorpusError::Geometry {
            id: id.to_owned(),
            reason: match e {
                negg_types::Error::Invalid(reason) => reason,
                other => other.to_string(),
            },
        })
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
    fn missing_frame_file_is_accepted_by_load() {
        // The corpus is local-only: a frame whose RAW is not on this
        // machine still loads, and `negg eval` skips it.
        let dir = full_corpus("missingframe");
        std::fs::remove_file(dir.0.join("frames/r001/r001-03.dng")).unwrap();
        let m = load(&dir.0).unwrap();
        assert_eq!(m.frame[0].file, "frames/r001/r001-03.dng");
    }

    #[test]
    fn missing_reference_file_is_accepted_by_load() {
        let dir = full_corpus("missingref");
        std::fs::remove_file(dir.0.join("reference/r001-03.negadoctor.tif")).unwrap();
        let m = load(&dir.0).unwrap();
        assert_eq!(m.frame[0].references.len(), 2);
    }

    #[test]
    fn manifest_with_no_files_at_all_loads() {
        let dir = TempDir::new("nofiles");
        dir.manifest(&format!("schema_version = 1\n{FRAME}"));
        assert_eq!(load(&dir.0).unwrap().frame.len(), 1);
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        let dir = full_corpus("traversal");
        for bad in ["../manifest.toml", "/etc/passwd", "./frames/x.dng", ""] {
            dir.manifest(&format!(
                "schema_version = 1\n{}",
                FRAME.replace("frames/r001/r001-03.dng", bad)
            ));
            match load(&dir.0) {
                Err(CorpusError::BadPath { id, file }) => {
                    assert_eq!(id, "r001-03");
                    assert_eq!(file, bad);
                }
                other => panic!("expected BadPath for {bad:?}, got {other:?}"),
            }
        }
        // The same rule applies to reference paths.
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace("reference/r001-03.negpy.tif", "../r001-03.negpy.tif")
        ));
        assert!(matches!(load(&dir.0), Err(CorpusError::BadPath { .. })));
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
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace("licence = \"CC0-1.0\"", "licence = \"LicenseRef-Private\"")
        ));
        assert!(load(&dir.0).is_ok());
    }

    #[test]
    fn the_committed_manifest_has_no_private_frames() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
        let manifest = load(&dir).expect("the committed manifest loads");
        let private: Vec<&str> = manifest
            .frame
            .iter()
            .filter(|f| f.licence == PRIVATE_LICENCE)
            .map(|f| f.id.as_str())
            .collect();
        assert!(
            private.is_empty(),
            "private frames belong in a corpus outside the repository: {private:?}"
        );
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

    /// `FRAME` as frame `id` of `roll` with extra fields appended.
    fn frame_with(id: &str, roll: &str, extra: &str) -> String {
        FRAME
            .replace("r001-03", id)
            .replace("roll = \"r001\"", &format!("roll = \"{roll}\""))
            .replace(
                "licence = \"CC0-1.0\"\n",
                &format!("licence = \"CC0-1.0\"\n{extra}\n"),
            )
    }

    #[test]
    fn gate2_fields_are_optional_and_load() {
        let dir = full_corpus("gate2fields");
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            frame_with(
                "r001-03",
                "r001",
                "film_roll = \"f001\"\nrig = \"../profiles/rig/a.rig.negg.toml\"\nfilm_profile = \"profiles/p.film.negg.toml\""
            )
        ));
        let m = load(&dir.0).unwrap();
        let f = &m.frame[0];
        assert_eq!(f.film_roll.as_deref(), Some("f001"));
        assert_eq!(f.rig.as_deref(), Some("../profiles/rig/a.rig.negg.toml"));
        assert_eq!(f.film_profile.as_deref(), Some("profiles/p.film.negg.toml"));
        // And they default to absent.
        dir.manifest(&format!("schema_version = 1\n{FRAME}"));
        let m = load(&dir.0).unwrap();
        assert!(m.frame[0].film_roll.is_none() && m.frame[0].rig.is_none());
        assert!(m.frame[0].film_profile.is_none());
    }

    #[test]
    fn frames_of_a_roll_share_stock_rig_and_film_profile() {
        let dir = full_corpus("rollshare");
        let a = frame_with("r001-01", "r001", "rig = \"a.rig.negg.toml\"");
        let b = frame_with("r001-02", "r001", "rig = \"b.rig.negg.toml\"");
        dir.manifest(&format!("schema_version = 1\n{a}\n{b}"));
        match load(&dir.0) {
            Err(e @ CorpusError::Inconsistent { .. }) => {
                let msg = e.to_string();
                assert!(msg.contains("r001-02") && msg.contains("rig"), "{msg}");
            }
            other => panic!("expected Inconsistent, got {other:?}"),
        }
        // One with a film profile and one without also disagree.
        let b = frame_with(
            "r001-02",
            "r001",
            "rig = \"a.rig.negg.toml\"\nfilm_profile = \"p\"",
        );
        dir.manifest(&format!("schema_version = 1\n{a}\n{b}"));
        assert!(matches!(
            load(&dir.0),
            Err(CorpusError::Inconsistent {
                field: "film_profile",
                ..
            })
        ));
        // So do two stocks on one roll.
        let b = frame_with("r001-02", "r001", "rig = \"a.rig.negg.toml\"")
            .replace("Kodak Portra 400", "Kodak Gold 200");
        dir.manifest(&format!("schema_version = 1\n{a}\n{b}"));
        match load(&dir.0) {
            Err(
                e @ CorpusError::Inconsistent {
                    group: "roll",
                    field: "stock",
                    ..
                },
            ) => {
                let msg = e.to_string();
                assert!(msg.contains("r001-02") && msg.contains("stock"), "{msg}");
            }
            other => panic!("expected Inconsistent stock, got {other:?}"),
        }
        // Different rolls may differ.
        let b = frame_with("r002-01", "r002", "rig = \"b.rig.negg.toml\"");
        dir.manifest(&format!("schema_version = 1\n{a}\n{b}"));
        assert!(load(&dir.0).is_ok());
    }

    #[test]
    fn frames_of_a_film_roll_share_stock() {
        let dir = full_corpus("filmroll");
        let a = frame_with("r001-01", "r001", "film_roll = \"f001\"");
        let b = frame_with("r002-01", "r002", "film_roll = \"f001\"")
            .replace("Kodak Portra 400", "Kodak Gold 200");
        dir.manifest(&format!("schema_version = 1\n{a}\n{b}"));
        assert!(matches!(
            load(&dir.0),
            Err(CorpusError::Inconsistent {
                group: "film_roll",
                field: "stock",
                ..
            })
        ));
        // Same stock across two scan rolls (two lights) is fine.
        let b = frame_with("r002-01", "r002", "film_roll = \"f001\"")
            .replace("Cinestill CS-Lite", "Kaiser Slimlite");
        dir.manifest(&format!("schema_version = 1\n{a}\n{b}"));
        assert!(load(&dir.0).is_ok());
    }

    #[test]
    fn profile_paths_must_be_relative() {
        let dir = full_corpus("profilepath");
        for bad in [
            "rig = \"\"",
            "rig = \"/etc/x.rig.negg.toml\"",
            "film_profile = \"/x\"",
        ] {
            dir.manifest(&format!(
                "schema_version = 1\n{}",
                frame_with("r001-03", "r001", bad)
            ));
            assert!(
                matches!(load(&dir.0), Err(CorpusError::BadProfilePath { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn nlp_tool_parses() {
        let dir = full_corpus("nlp");
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            FRAME.replace(
                "tool = \"negpy\", file = \"reference/r001-03.negpy.tif\"",
                "tool = \"nlp\", file = \"reference/r001-03.nlp.jpg\""
            )
        ));
        let m = load(&dir.0).unwrap();
        assert_eq!(m.frame[0].references[0].tool, Tool::Nlp);
        assert_eq!(Tool::Nlp.to_string(), "nlp");
        assert_eq!(serde_json::to_string(&Tool::Nlp).unwrap(), "\"nlp\"");
    }

    #[test]
    fn geometry_is_optional_loads_and_defaults_like_settings() {
        let dir = full_corpus("geometry");
        let m = load(&dir.0).unwrap();
        assert!(m.frame[0].geometry.is_none());
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            frame_with(
                "r001-03",
                "r001",
                "geometry = { crop = { x = 0.1, y = 0.05, width = 0.8, height = 0.9 }, rotate_degrees = 90, flip_horizontal = true }"
            )
        ));
        let m = load(&dir.0).unwrap();
        let g = m.frame[0].geometry.unwrap().to_geometry();
        assert_eq!(
            g.crop,
            Some(negg_types::CropRect {
                x: 0.1,
                y: 0.05,
                width: 0.8,
                height: 0.9
            })
        );
        assert_eq!(g.analysis_buffer, negg_types::ANALYSIS_BUFFER_DEFAULT);
        assert_eq!((g.rotate_degrees, g.flip_horizontal), (90, true));
        // An empty table is the default geometry, the identity.
        dir.manifest(&format!(
            "schema_version = 1\n{}",
            frame_with("r001-03", "r001", "geometry = {}")
        ));
        let m = load(&dir.0).unwrap();
        assert_eq!(
            m.frame[0].geometry.unwrap().to_geometry(),
            negg_types::Geometry::default()
        );
    }

    #[test]
    fn bad_geometry_is_rejected() {
        let dir = full_corpus("badgeometry");
        for (bad, needle) in [
            (
                "geometry = { crop = { x = 0.5, y = 0.0, width = 0.6, height = 1.0 } }",
                "x + width",
            ),
            (
                "geometry = { crop = { x = 0.0, y = 0.0, width = 0.005, height = 1.0 } }",
                "width",
            ),
            ("geometry = { rotate_degrees = 45 }", "rotate_degrees"),
            ("geometry = { analysis_buffer = 0.3 }", "analysis_buffer"),
        ] {
            dir.manifest(&format!(
                "schema_version = 1\n{}",
                frame_with("r001-03", "r001", bad)
            ));
            match load(&dir.0) {
                Err(e @ CorpusError::Geometry { .. }) => {
                    let msg = e.to_string();
                    assert!(msg.contains("r001-03") && msg.contains(needle), "{msg}");
                }
                other => panic!("expected Geometry for {bad}, got {other:?}"),
            }
        }
        // Unknown keys fail to parse, in the geometry and in its crop.
        for bad in [
            "geometry = { rotate = 90 }",
            "geometry = { crop = { x = 0.0, y = 0.0, width = 1.0, height = 1.0, w = 1 } }",
            "geometry = { crop = { x = 0.0, y = 0.0, width = 1.0 } }",
        ] {
            dir.manifest(&format!(
                "schema_version = 1\n{}",
                frame_with("r001-03", "r001", bad)
            ));
            assert!(matches!(load(&dir.0), Err(CorpusError::Parse(_))), "{bad}");
        }
    }
}
