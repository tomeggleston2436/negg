// SPDX-License-Identifier: MPL-2.0

//! TOML reading and writing for rig and film profiles (docs/PROFILES.md).
//!
//! The types and `validate` live in negg-types; this module only moves them
//! to and from `*.negg.toml` files. Both directions validate, so an invalid
//! profile (for example a `TODO(measure)` source) is never loaded or
//! written. Profile data feeds steps 2 to 4 of docs/MATHS.md; no maths is
//! done here.

use std::fs;
use std::path::Path;

use negg_types::{FilmProfile, RigProfile};

use crate::error::{IoError, io_at};
use crate::sidecar::write_atomic;

/// Reads a rig profile from TOML and validates it (docs/PROFILES.md, rig
/// profile; feeds step 2 of docs/MATHS.md).
///
/// # Errors
///
/// [`IoError::Io`] for file errors and [`IoError::Profile`] for TOML errors
/// (including unknown fields) and validation failures.
pub fn read_rig_profile(path: &Path) -> Result<RigProfile, IoError> {
    let text = fs::read_to_string(path).map_err(io_at(path))?;
    let profile: RigProfile =
        toml::from_str(&text).map_err(|e| IoError::Profile(format!("{}: {e}", path.display())))?;
    profile
        .validate()
        .map_err(|e| IoError::Profile(format!("{}: {e}", path.display())))?;
    Ok(profile)
}

/// Validates then writes a rig profile as TOML, atomically (docs/PROFILES.md,
/// rig profile; step 2 of docs/MATHS.md).
///
/// # Errors
///
/// [`IoError::Profile`] when validation or serialisation fails,
/// [`IoError::Io`] for file errors.
pub fn write_rig_profile(path: &Path, profile: &RigProfile) -> Result<(), IoError> {
    profile
        .validate()
        .map_err(|e| IoError::Profile(e.to_string()))?;
    let text = toml::to_string_pretty(profile).map_err(|e| IoError::Profile(e.to_string()))?;
    write_atomic(path, text.as_bytes())
}

/// Reads a film profile from TOML and validates it (docs/PROFILES.md, film
/// profile; feeds steps 3 and 4 of docs/MATHS.md).
///
/// # Errors
///
/// As [`read_rig_profile`].
pub fn read_film_profile(path: &Path) -> Result<FilmProfile, IoError> {
    let text = fs::read_to_string(path).map_err(io_at(path))?;
    let profile: FilmProfile =
        toml::from_str(&text).map_err(|e| IoError::Profile(format!("{}: {e}", path.display())))?;
    profile
        .validate()
        .map_err(|e| IoError::Profile(format!("{}: {e}", path.display())))?;
    Ok(profile)
}

/// Validates then writes a film profile as TOML, atomically
/// (docs/PROFILES.md, film profile; steps 3 and 4 of docs/MATHS.md).
///
/// # Errors
///
/// As [`write_rig_profile`].
pub fn write_film_profile(path: &Path, profile: &FilmProfile) -> Result<(), IoError> {
    profile
        .validate()
        .map_err(|e| IoError::Profile(e.to_string()))?;
    let text = toml::to_string_pretty(profile).map_err(|e| IoError::Profile(e.to_string()))?;
    write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_types::profile::{
        BaseTable, CrosstalkTable, DensityFitTable, DensityModelKind, DmaxTable, FilmMethod,
        FlatFieldTable, GammaTable, RigMethod, StockTable,
    };
    use std::path::PathBuf;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "negg-io-profile-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn rig() -> RigProfile {
        RigProfile {
            schema_version: 1,
            name: "test rig".into(),
            source: "synthetic unit test data".into(),
            measured_on: "2026-01-02".into(),
            method: vec![
                RigMethod::FlatField,
                RigMethod::Crosstalk,
                RigMethod::DensityFit,
            ],
            residual_rms: 0.0125,
            camera: Some("synthetic camera".into()),
            flat_field: Some(FlatFieldTable {
                method: RigMethod::FlatField,
                source: "synthetic flat".into(),
                measured_on: "2026-01-02".into(),
                frame: "flat.dng".into(),
                mean: [0.25, 0.5, 0.75],
                blur_sigma: "long_side / 64".into(),
                width: 600,
                height: 400,
            }),
            crosstalk: Some(CrosstalkTable {
                method: RigMethod::Crosstalk,
                source: "synthetic bands".into(),
                measured_on: "2026-01-02".into(),
                frame_r: "r.dng".into(),
                frame_g: "g.dng".into(),
                frame_b: "b.dng".into(),
                m: [[1.0, 0.125, 0.0], [0.0625, 1.0, 0.03125], [0.0, 0.25, 1.0]],
            }),
            density_fit: Some(DensityFitTable {
                method: RigMethod::DensityFit,
                source: "synthetic wedge".into(),
                measured_on: "2026-01-02".into(),
                residual_rms: 0.0125,
                n_patches: 21,
                model: DensityModelKind::Full,
                a: [0.5, 0.25, 0.125],
                b: [-1.0, -0.5, -0.75],
                c: [[0.0, 0.125, 0.0], [0.0625, 0.0, 0.0], [0.0, 0.25, 0.0]],
            }),
        }
    }

    fn film() -> FilmProfile {
        FilmProfile {
            schema_version: 1,
            name: "test stock".into(),
            source: "synthetic roll".into(),
            measured_on: "2026-01-02".into(),
            method: FilmMethod::GreyCardRoll,
            residual_rms: 0.015625,
            rig: Some("test rig".into()),
            stock: StockTable {
                name: "test stock".into(),
                process: "C-41".into(),
                iso: 400,
            },
            base: Some(BaseTable {
                dmin: [0.25, 0.5, 0.75],
                source: "synthetic".into(),
            }),
            dmax: Some(DmaxTable {
                dmax: [2.0, 2.25, 2.5],
                source: "synthetic".into(),
            }),
            gamma: Some(GammaTable {
                gamma: [0.5, 0.5, 0.5],
                ratio: [1.125, 1.0, 0.875],
                offset: [0.03125, 0.0, -0.015625],
                gamma_source: "neutral_fit".into(),
                n_patches: 7,
                source: "synthetic".into(),
            }),
        }
    }

    #[test]
    fn rig_profile_round_trips() {
        let dir = TempDir::new("rig");
        let path = dir.path("t.rig.negg.toml");
        write_rig_profile(&path, &rig()).unwrap();
        assert_eq!(read_rig_profile(&path).unwrap(), rig());
        assert!(!dir.path("t.rig.negg.toml.tmp").exists());
    }

    #[test]
    fn rig_profile_with_no_tables_round_trips() {
        let dir = TempDir::new("rig-empty");
        let path = dir.path("e.rig.negg.toml");
        let mut p = rig();
        p.method.clear();
        p.flat_field = None;
        p.crosstalk = None;
        p.density_fit = None;
        p.residual_rms = 0.0;
        p.camera = None;
        write_rig_profile(&path, &p).unwrap();
        assert_eq!(read_rig_profile(&path).unwrap(), p);
    }

    #[test]
    fn film_profile_round_trips() {
        let dir = TempDir::new("film");
        let path = dir.path("t.film.negg.toml");
        write_film_profile(&path, &film()).unwrap();
        assert_eq!(read_film_profile(&path).unwrap(), film());
    }

    #[test]
    fn film_stub_with_method_none_round_trips() {
        let dir = TempDir::new("film-none");
        let path = dir.path("n.film.negg.toml");
        let mut p = film();
        p.method = FilmMethod::None;
        p.base = None;
        p.dmax = None;
        p.gamma = None;
        p.rig = None;
        p.residual_rms = 0.0;
        write_film_profile(&path, &p).unwrap();
        assert_eq!(read_film_profile(&path).unwrap(), p);
    }

    #[test]
    fn writers_validate_and_leave_no_file() {
        let dir = TempDir::new("invalid");
        let mut bad = rig();
        bad.source = "TODO(measure): later".into();
        let path = dir.path("bad.rig.negg.toml");
        assert!(matches!(
            write_rig_profile(&path, &bad),
            Err(IoError::Profile(_))
        ));
        assert!(!path.exists());
        let mut bad = film();
        bad.measured_on = "yesterday".into();
        let path = dir.path("bad.film.negg.toml");
        assert!(matches!(
            write_film_profile(&path, &bad),
            Err(IoError::Profile(_))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn readers_reject_the_profiles_md_examples() {
        // The worked examples in docs/PROFILES.md carry TODO(measure).
        let dir = TempDir::new("todo");
        let mut p = rig();
        p.source = "TODO(measure): describe the camera".into();
        let text = toml::to_string_pretty(&p).unwrap();
        let path = dir.path("x.rig.negg.toml");
        fs::write(&path, text).unwrap();
        match read_rig_profile(&path) {
            Err(IoError::Profile(m)) => assert!(m.contains("TODO(measure)"), "{m}"),
            other => panic!("expected Profile error, got {other:?}"),
        }
        let mut f = film();
        f.source = "TODO(measure): roll".into();
        let path = dir.path("x.film.negg.toml");
        fs::write(&path, toml::to_string_pretty(&f).unwrap()).unwrap();
        assert!(matches!(read_film_profile(&path), Err(IoError::Profile(_))));
    }

    #[test]
    fn readers_reject_unknown_fields_and_bad_toml_and_missing_files() {
        let dir = TempDir::new("bad");
        let mut text = toml::to_string_pretty(&film()).unwrap();
        text.insert_str(0, "colour = 1\n");
        let path = dir.path("u.film.negg.toml");
        fs::write(&path, text).unwrap();
        assert!(matches!(read_film_profile(&path), Err(IoError::Profile(_))));
        fs::write(&path, "not = [toml").unwrap();
        assert!(matches!(read_film_profile(&path), Err(IoError::Profile(_))));
        assert!(matches!(
            read_rig_profile(&dir.path("absent.rig.negg.toml")),
            Err(IoError::Io { .. })
        ));
    }
}
