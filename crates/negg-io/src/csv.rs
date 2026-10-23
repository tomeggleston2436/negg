// SPDX-License-Identifier: MPL-2.0

//! Readers for the calibration input files: `wedge.csv` and `patches.csv`
//! (docs/PROFILES.md, Input CSV files; Ruling 42).
//!
//! Both are UTF-8, comma separated, with no quoting. Blank lines and lines
//! starting with `#` are ignored; the first other line is the header. A
//! malformed line is an [`IoError::Csv`] naming its one-based line number.
//! Rectangles are fractions of the frame and must lie inside [0, 1]. The
//! wedge densities feed the density fit of step 2 of docs/MATHS.md and the
//! patch list feeds the neutral-patch fit of step 4.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use negg_types::PatchRect;

use crate::error::{IoError, io_at};

/// One patch of a step wedge: label, location and certified density.
#[derive(Clone, Debug, PartialEq)]
pub struct WedgePatch {
    /// Unique patch label.
    pub label: String,
    /// Location of the patch as fractions of the frame.
    pub rect: PatchRect,
    /// Certified transmission density per channel (R, G, B). A neutral
    /// wedge repeats its single density in all three.
    pub density: [f32; 3],
}

/// One neutral patch on a roll frame, for `negg profile film`.
#[derive(Clone, Debug, PartialEq)]
pub struct NeutralPatchRef {
    /// File name in the roll directory.
    pub file: String,
    /// Location of the patch as fractions of the frame.
    pub rect: PatchRect,
}

const WEDGE_NEUTRAL_HEADER: &str = "patch,x,y,width,height,density";
const WEDGE_COLOUR_HEADER: &str = "patch,x,y,width,height,density_r,density_g,density_b";
const PATCHES_HEADER: &str = "file,x,y,width,height";

fn err(line: usize, message: impl Into<String>) -> IoError {
    IoError::Csv {
        line,
        message: message.into(),
    }
}

/// Yields `(line_number, trimmed_line)` for every line that is neither blank
/// nor a comment. A leading UTF-8 byte order mark (U+FEFF), which some
/// spreadsheet programs write, is dropped first so the header still matches.
fn data_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
}

fn parse_f32(line: usize, name: &str, text: &str) -> Result<f32, IoError> {
    let v: f32 = text
        .trim()
        .parse()
        .map_err(|_| err(line, format!("{name} {text:?} is not a number")))?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(err(line, format!("{name} must be finite")))
    }
}

fn parse_rect(line: usize, fields: &[&str]) -> Result<PatchRect, IoError> {
    let rect = PatchRect {
        x: parse_f32(line, "x", fields[0])?,
        y: parse_f32(line, "y", fields[1])?,
        width: parse_f32(line, "width", fields[2])?,
        height: parse_f32(line, "height", fields[3])?,
    };
    rect.validate().map_err(|e| err(line, e.to_string()))?;
    Ok(rect)
}

fn check_label(line: usize, name: &str, label: &str) -> Result<(), IoError> {
    if label.trim().is_empty() {
        Err(err(line, format!("{name} must not be empty")))
    } else {
        Ok(())
    }
}

/// Parses `wedge.csv` text (Ruling 42; step 2 of docs/MATHS.md).
///
/// The header is exactly `patch,x,y,width,height,density` (neutral) or
/// `patch,x,y,width,height,density_r,density_g,density_b` (coloured).
/// Labels must be unique, rectangles inside [0, 1], densities finite and at
/// least 0. At least one patch is required.
///
/// # Errors
///
/// [`IoError::Csv`] naming the offending line.
pub fn parse_wedge_csv(text: &str) -> Result<Vec<WedgePatch>, IoError> {
    let mut lines = data_lines(text);
    let (header_line, header) = lines
        .next()
        .ok_or_else(|| err(1, "no header line; expected a wedge CSV header"))?;
    let columns = if header == WEDGE_NEUTRAL_HEADER {
        6
    } else if header == WEDGE_COLOUR_HEADER {
        8
    } else {
        return Err(err(
            header_line,
            format!("header must be exactly `{WEDGE_NEUTRAL_HEADER}` or `{WEDGE_COLOUR_HEADER}`"),
        ));
    };
    let mut seen = HashSet::new();
    let mut patches = Vec::new();
    for (n, line) in lines {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() != columns {
            return Err(err(
                n,
                format!("expected {columns} fields, found {}", fields.len()),
            ));
        }
        let label = fields[0];
        check_label(n, "patch", label)?;
        if !seen.insert(label.to_owned()) {
            return Err(err(n, format!("duplicate patch label {label:?}")));
        }
        let rect = parse_rect(n, &fields[1..5])?;
        let mut density = [0.0_f32; 3];
        if columns == 6 {
            let d = parse_f32(n, "density", fields[5])?;
            density = [d; 3];
        } else {
            for (c, name) in ["density_r", "density_g", "density_b"].iter().enumerate() {
                density[c] = parse_f32(n, name, fields[5 + c])?;
            }
        }
        if density.iter().any(|d| *d < 0.0) {
            return Err(err(n, "densities must be at least 0"));
        }
        patches.push(WedgePatch {
            label: label.to_owned(),
            rect,
            density,
        });
    }
    if patches.is_empty() {
        return Err(err(header_line, "no patches after the header"));
    }
    Ok(patches)
}

/// Reads and parses a `wedge.csv` file (Ruling 42; step 2 of
/// docs/MATHS.md).
///
/// # Errors
///
/// [`IoError::Io`] for file errors, otherwise as [`parse_wedge_csv`].
pub fn read_wedge_csv(path: &Path) -> Result<Vec<WedgePatch>, IoError> {
    let text = fs::read_to_string(path).map_err(io_at(path))?;
    parse_wedge_csv(&text)
}

/// Parses `patches.csv` text (Ruling 42; step 4 of docs/MATHS.md).
///
/// The header is exactly `file,x,y,width,height`; `file` is a bare file
/// name in the roll directory (no directory separators). Several patches
/// may name the same file. At least one patch is required.
///
/// # Errors
///
/// [`IoError::Csv`] naming the offending line.
pub fn parse_patches_csv(text: &str) -> Result<Vec<NeutralPatchRef>, IoError> {
    let mut lines = data_lines(text);
    let (header_line, header) = lines
        .next()
        .ok_or_else(|| err(1, "no header line; expected a patches CSV header"))?;
    if header != PATCHES_HEADER {
        return Err(err(
            header_line,
            format!("header must be exactly `{PATCHES_HEADER}`"),
        ));
    }
    let mut patches = Vec::new();
    for (n, line) in lines {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() != 5 {
            return Err(err(n, format!("expected 5 fields, found {}", fields.len())));
        }
        let file = fields[0];
        check_label(n, "file", file)?;
        if file.contains(['/', '\\']) {
            return Err(err(
                n,
                format!("file must be a name in the roll directory, got {file:?}"),
            ));
        }
        patches.push(NeutralPatchRef {
            file: file.to_owned(),
            rect: parse_rect(n, &fields[1..5])?,
        });
    }
    if patches.is_empty() {
        return Err(err(header_line, "no patches after the header"));
    }
    Ok(patches)
}

/// Reads and parses a `patches.csv` file (Ruling 42; step 4 of
/// docs/MATHS.md).
///
/// # Errors
///
/// [`IoError::Io`] for file errors, otherwise as [`parse_patches_csv`].
pub fn read_patches_csv(path: &Path) -> Result<Vec<NeutralPatchRef>, IoError> {
    let text = fs::read_to_string(path).map_err(io_at(path))?;
    parse_patches_csv(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_of(r: Result<impl std::fmt::Debug, IoError>) -> usize {
        match r {
            Err(IoError::Csv { line, .. }) => line,
            other => panic!("expected Csv error, got {other:?}"),
        }
    }

    #[test]
    fn neutral_wedge_repeats_density_in_all_channels() {
        let text = "# certified wedge\n\npatch,x,y,width,height,density\n\
                    p1,0.1,0.2,0.05,0.05,0.25\np2, 0.3 ,0.2,0.05,0.05,1.5\n";
        let p = parse_wedge_csv(text).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].label, "p1");
        assert_eq!(
            p[0].rect,
            PatchRect {
                x: 0.1,
                y: 0.2,
                width: 0.05,
                height: 0.05
            }
        );
        assert_eq!(p[0].density, [0.25; 3]);
        assert_eq!(p[1].rect.x, 0.3);
        assert_eq!(p[1].density, [1.5; 3]);
    }

    #[test]
    fn a_byte_order_mark_before_the_header_is_ignored() {
        let wedge =
            parse_wedge_csv("\u{feff}patch,x,y,width,height,density\na,0,0,1,1,0.5\n").unwrap();
        assert_eq!(wedge[0].density, [0.5; 3]);
        let patches = parse_patches_csv("\u{feff}file,x,y,width,height\ng.tif,0,0,1,1\n").unwrap();
        assert_eq!(patches.len(), 1);
        // A mark anywhere else is still an error.
        assert!(parse_patches_csv("# c\n\u{feff}file,x,y,width,height\ng.tif,0,0,1,1\n").is_err());
    }

    #[test]
    fn coloured_wedge_reads_three_densities() {
        let text = "patch,x,y,width,height,density_r,density_g,density_b\n\
                    a,0,0,1,1,0.1,0.2,0.3\n";
        let p = parse_wedge_csv(text).unwrap();
        assert_eq!(p[0].density, [0.1, 0.2, 0.3]);
        assert_eq!(p[0].rect.width, 1.0);
    }

    #[test]
    fn wedge_errors_name_the_line() {
        let head = "patch,x,y,width,height,density\n";
        // Header on line 3 after comment and blank.
        assert_eq!(line_of(parse_wedge_csv("#c\n\nwrong,header\n")), 3);
        assert_eq!(line_of(parse_wedge_csv("")), 1);
        assert_eq!(line_of(parse_wedge_csv(head)), 1);
        let cases: [(&str, usize); 8] = [
            ("p,0.1,0.1,0.1,0.1\n", 2),
            ("p,0.1,0.1,0.1,0.1,0.5,9\n", 2),
            ("p,abc,0.1,0.1,0.1,0.5\n", 2),
            ("p,0.1,0.1,0.1,0.1,-0.5\n", 2),
            ("p,0.1,0.1,0.1,0.1,NaN\n", 2),
            ("p,0.95,0.1,0.1,0.1,0.5\n", 2),
            ("p,0.1,0.1,0,0.1,0.5\n", 2),
            (",0.1,0.1,0.1,0.1,0.5\n", 2),
        ];
        for (body, line) in cases {
            assert_eq!(
                line_of(parse_wedge_csv(&format!("{head}{body}"))),
                line,
                "{body}"
            );
        }
        let dup = format!("{head}a,0,0,0.1,0.1,1\n# note\nb,0,0,0.1,0.1,1\na,0,0,0.1,0.1,1\n");
        assert_eq!(line_of(parse_wedge_csv(&dup)), 5);
    }

    #[test]
    fn rect_must_lie_inside_the_unit_square() {
        let head = "patch,x,y,width,height,density\n";
        for body in [
            "p,-0.1,0,0.1,0.1,1\n",
            "p,0,0,1.1,0.1,1\n",
            "p,0.5,0.5,0.6,0.1,1\n",
        ] {
            assert_eq!(line_of(parse_wedge_csv(&format!("{head}{body}"))), 2);
        }
    }

    #[test]
    fn patches_csv_parses_and_rejects() {
        let text = "# neutral patches\nfile,x,y,width,height\n\
                    a.dng,0.4,0.4,0.1,0.1\na.dng,0.1,0.1,0.1,0.1\nb.dng,0.2,0.2,0.2,0.2\n";
        let p = parse_patches_csv(text).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].file, "a.dng");
        assert_eq!(p[2].rect.width, 0.2);

        let head = "file,x,y,width,height\n";
        assert_eq!(line_of(parse_patches_csv("patch,x,y,width,height\n")), 1);
        assert_eq!(line_of(parse_patches_csv(head)), 1);
        assert_eq!(
            line_of(parse_patches_csv(&format!("{head}a.dng,0.1,0.1,0.1\n"))),
            2
        );
        assert_eq!(
            line_of(parse_patches_csv(&format!(
                "{head}sub/a.dng,0.1,0.1,0.1,0.1\n"
            ))),
            2
        );
        assert_eq!(
            line_of(parse_patches_csv(&format!(
                "{head}a.dng,0.1,0.1,0.1,0.1\nb.dng,2,0,0.1,0.1\n"
            ))),
            3
        );
    }

    #[test]
    fn read_functions_read_files_and_report_missing_ones() {
        let dir = std::env::temp_dir().join(format!("negg-io-csv-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let w = dir.join("wedge.csv");
        fs::write(&w, "patch,x,y,width,height,density\na,0,0,0.5,0.5,1\n").unwrap();
        assert_eq!(read_wedge_csv(&w).unwrap().len(), 1);
        let p = dir.join("patches.csv");
        fs::write(&p, "file,x,y,width,height\na.dng,0,0,0.5,0.5\n").unwrap();
        assert_eq!(read_patches_csv(&p).unwrap().len(), 1);
        assert!(matches!(
            read_wedge_csv(&dir.join("none.csv")),
            Err(IoError::Io { .. })
        ));
        assert!(matches!(
            read_patches_csv(&dir.join("none.csv")),
            Err(IoError::Io { .. })
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
