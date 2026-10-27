// SPDX-License-Identifier: Apache-2.0

//! Command-line definition for the `negg` binary.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use negg_types::CropRect;
use negg_types::settings::OutputSpace;

/// The `negg` film negative converter.
#[derive(Debug, Parser)]
#[command(name = "negg", version, about)]
pub struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Convert one negative to a positive, writing the image and a sidecar.
    Convert(ConvertArgs),
    /// Convert every RAW or TIFF negative in a directory as one roll
    /// (pooled film base, gamma and cast) unless `--no-roll` is given.
    Batch(BatchArgs),
    /// Render every corpus frame and score it against the reference renders.
    Eval(EvalArgs),
    /// Measure a rig or film profile.
    #[command(subcommand)]
    Profile(ProfileCommand),
    /// Lock a converted frame's current estimates into its sidecar
    /// overrides, so later renders keep them.
    Lock(LockArgs),
    /// Tone presets: the built-in ones and the user's
    /// `*.preset.negg.toml` files in the presets folder.
    #[command(subcommand)]
    Preset(PresetCommand),
}

/// `negg preset` subcommands.
#[derive(Debug, Subcommand)]
pub enum PresetCommand {
    /// List the presets `--preset` can name, one per line: the name, a tab,
    /// then `(built-in)` or the file it was read from. The user presets
    /// folder is `$NEGG_PRESETS_DIR` when set, else `negg/presets` in the
    /// platform config folder. Files that cannot be used are skipped with a
    /// warning.
    List,
}

/// Output file formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum Format {
    /// 16-bit TIFF with Deflate compression and the ICC profile embedded.
    Tiff16,
    /// 8-bit JPEG at quality 92 with the ICC profile embedded.
    Jpeg,
}

impl Format {
    /// The file extension written for this format.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Tiff16 => "tif",
            Format::Jpeg => "jpg",
        }
    }
}

/// Output colour spaces, mirroring [`OutputSpace`] with the names the
/// settings file uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum Space {
    /// sRGB (IEC 61966-2-1).
    Srgb,
    /// Adobe RGB (1998).
    AdobeRgb,
    /// Display P3.
    DisplayP3,
    /// ProPhoto RGB (ROMM).
    ProPhoto,
    /// ITU-R BT.2020.
    Rec2020,
}

impl From<Space> for OutputSpace {
    fn from(space: Space) -> OutputSpace {
        match space {
            Space::Srgb => OutputSpace::Srgb,
            Space::AdobeRgb => OutputSpace::AdobeRgb,
            Space::DisplayP3 => OutputSpace::DisplayP3,
            Space::ProPhoto => OutputSpace::ProPhoto,
            Space::Rec2020 => OutputSpace::Rec2020,
        }
    }
}

/// Options shared by `negg convert` and `negg batch`.
#[derive(Debug, Args)]
pub struct OutputArgs {
    /// Directory to write the output image (`<stem>.negg.tif` or
    /// `<stem>.negg.jpg`) into. Defaults to the input's own directory.
    /// Created if it does not exist.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// A JSON settings file (the `Settings` schema; omitted fields take
    /// their defaults), a frame sidecar (its `settings` are used) or a roll
    /// sidecar `roll.negg.json` (its shared settings are used); anything
    /// else, or a file of a newer schema than this negg, is refused. Without
    /// it the settings are the built-in defaults, or in roll mode the roll
    /// settings stored in `roll.negg.json`. In roll mode the file replaces
    /// the stored look (film, print, colour, profiles) and keeps the stored
    /// geometry (crop, analysis area, buffer, rotation, flip) unless it has
    /// a `geometry` key; `negg batch` warns when it replaces stored
    /// settings and stores the result in `roll.negg.json`. `--preset`
    /// applies after it.
    #[arg(long, value_name = "FILE")]
    pub settings: Option<PathBuf>,
    /// A tone preset: a built-in or user preset name (case-insensitive;
    /// `negg preset list` shows them) or a `*.preset.negg.toml` file. It
    /// sets grade, toe, shoulder, black point compensation and saturation,
    /// and the preset label, after `--settings`. Without roll mode it sets
    /// them on the frame's settings. `negg convert` of a frame in a roll
    /// sets them as the frame's own look, in its sidecar overrides. `negg
    /// batch` in roll mode sets them as the roll's look, stored in
    /// `roll.negg.json`; a frame with its own look keeps rendering exactly
    /// as before (its whole current look is written into its sidecar
    /// overrides) unless `--preset-all-frames` is given.
    #[arg(long, value_name = "NAME|FILE")]
    pub preset: Option<String>,
    /// With `--preset` in `negg batch` roll mode: clear the own look of
    /// frames that have one, so every frame takes the preset. Their other
    /// overrides stay.
    #[arg(long, requires = "preset")]
    pub preset_all_frames: bool,
    /// Output file format.
    #[arg(long, value_enum, default_value_t = Format::Tiff16)]
    pub format: Format,
    /// Also write a web copy of each frame to `<out>/web/<stem>.negg.jpg`:
    /// an sRGB JPEG (whatever `--space` says) at quality 85, 2048 px on the
    /// long edge and never upscaled, with the same framing as the output.
    #[arg(long, conflicts_with = "web_only")]
    pub web: bool,
    /// Write only the web copy (as `--web`), no full-size output. The
    /// sidecars are written as for any conversion.
    #[arg(long)]
    pub web_only: bool,
    /// Output colour space. Defaults to the settings' `colour.output_space`
    /// (sRGB unless a settings file says otherwise).
    #[arg(long, value_enum)]
    pub space: Option<Space>,
    /// Crop as `x,y,w,h`: four fractions of the frame in its source
    /// orientation (before any rotation). The crop frames the export, and
    /// it also limits what the analysis sees unless an analysis area is set
    /// (`--analysis-area`, or one stored for the roll). In roll mode it is
    /// the roll's shared crop for the run; `negg batch` stores it in
    /// `roll.negg.json` (`negg convert` never writes the roll sidecar).
    #[arg(long, value_name = "X,Y,W,H", value_parser = parse_crop)]
    pub crop: Option<CropRect>,
    /// Analysis area as `x,y,w,h`: four fractions of the frame in its
    /// source orientation, like `--crop`. When given, only this area is
    /// analysed instead of the crop, and the crop, if any, frames the export
    /// alone. Cover the whole film including its clear edge (rebate) and
    /// leave out only the film holder: the clear edge is where the film
    /// base is measured. In roll mode it is the roll's shared analysis area
    /// for the run; `negg batch` stores it in `roll.negg.json` (`negg
    /// convert` never writes the roll sidecar). A frame whose sidecar has
    /// its own geometry keeps its own.
    #[arg(long, value_name = "X,Y,W,H", value_parser = parse_crop)]
    pub analysis_area: Option<CropRect>,
    /// Print the estimates (Dmin, Dmax, gamma, anchor, x0, cast) and the
    /// grid and image sizes to stdout after each frame.
    #[arg(long)]
    pub dump_stats: bool,
}

/// Parses `--crop x,y,w,h` and `--analysis-area x,y,w,h`:
/// four comma-separated numbers that form a valid
/// [`CropRect`].
///
/// # Errors
///
/// A message naming the problem when there are not four numbers or the
/// rectangle fails [`CropRect::validate`].
pub fn parse_crop(text: &str) -> Result<CropRect, String> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return Err(format!(
            "expected four comma-separated fractions x,y,w,h, got {text:?}"
        ));
    }
    let mut v = [0f32; 4];
    for (slot, part) in v.iter_mut().zip(&parts) {
        *slot = part
            .parse::<f32>()
            .map_err(|e| format!("{part:?} is not a number: {e}"))?;
    }
    let rect = CropRect {
        x: v[0],
        y: v[1],
        width: v[2],
        height: v[3],
    };
    rect.validate().map_err(|e| e.to_string())?;
    Ok(rect)
}

/// Profile and roll options shared by `negg convert` and `negg batch`.
#[derive(Debug, Default, Args)]
pub struct RollArgs {
    /// A rig profile (`*.rig.negg.toml`) to correct the frames with. Stored
    /// in the settings as an absolute path.
    #[arg(long, value_name = "FILE")]
    pub rig: Option<PathBuf>,
    /// A film profile (`*.film.negg.toml`) whose gamma ratios and neutral
    /// offsets to apply. Stored in the settings as an absolute path.
    #[arg(long, value_name = "FILE")]
    pub film: Option<PathBuf>,
    /// Do not use roll mode: each frame is rendered on its own, as phase 1
    /// did, and no roll sidecar is read or written.
    #[arg(long)]
    pub no_roll: bool,
}

/// Arguments for `negg convert`.
#[derive(Debug, Args)]
pub struct ConvertArgs {
    /// The negative: a RAW file or a TIFF.
    pub input: PathBuf,
    /// Output options.
    #[command(flatten)]
    pub output: OutputArgs,
    /// Profile and roll options.
    #[command(flatten)]
    pub roll: RollArgs,
    /// Print per-stage times (decode including linearisation, analyse and
    /// render, transform, write) to stderr.
    #[arg(long)]
    pub timing: bool,
}

/// Arguments for `negg batch`.
#[derive(Debug, Args)]
pub struct BatchArgs {
    /// Directory holding the negatives. Not searched recursively.
    pub dir: PathBuf,
    /// Output options.
    #[command(flatten)]
    pub output: OutputArgs,
    /// Profile and roll options.
    #[command(flatten)]
    pub roll: RollArgs,
}

/// `negg profile` subcommands.
#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// Measure a rig profile from a flat frame, three band frames and a
    /// step wedge (any subset).
    Rig(ProfileRigArgs),
    /// Measure a film profile (gamma ratios and neutral offsets) from a roll
    /// with grey patches.
    Film(ProfileFilmArgs),
}

/// Provenance options shared by both profile commands.
#[derive(Debug, Args)]
pub struct ProvenanceArgs {
    /// The profile name.
    #[arg(long)]
    pub name: String,
    /// How the profile was measured (equipment, target, procedure).
    #[arg(long)]
    pub source: String,
    /// The profile file to write.
    #[arg(long, value_name = "FILE")]
    pub out: PathBuf,
    /// Measurement date, `YYYY-MM-DD`. Defaults to today's UTC date.
    #[arg(long, value_name = "DATE")]
    pub measured_on: Option<String>,
}

/// Arguments for `negg profile rig`.
#[derive(Debug, Args)]
pub struct ProfileRigArgs {
    /// Name, source, output and date.
    #[command(flatten)]
    pub provenance: ProvenanceArgs,
    /// A blank-light frame for the flat field (linear: RAW or float TIFF).
    #[arg(long, value_name = "FILE")]
    pub flat: Option<PathBuf>,
    /// The frame lit by the red band alone.
    #[arg(long, value_name = "FILE")]
    pub band_r: Option<PathBuf>,
    /// The frame lit by the green band alone.
    #[arg(long, value_name = "FILE")]
    pub band_g: Option<PathBuf>,
    /// The frame lit by the blue band alone.
    #[arg(long, value_name = "FILE")]
    pub band_b: Option<PathBuf>,
    /// A frame of a step wedge.
    #[arg(long, value_name = "FILE")]
    pub wedge: Option<PathBuf>,
    /// The wedge patches and their certified densities (`wedge.csv`).
    #[arg(long, value_name = "CSV")]
    pub densities: Option<PathBuf>,
}

/// Arguments for `negg profile film`.
#[derive(Debug, Args)]
pub struct ProfileFilmArgs {
    /// Directory holding the roll's negatives.
    pub dir: PathBuf,
    /// The grey patches (`patches.csv`: file, x, y, width, height).
    #[arg(long, value_name = "CSV")]
    pub patches: PathBuf,
    /// Name, source, output and date.
    #[command(flatten)]
    pub provenance: ProvenanceArgs,
    /// The stock name as printed on the box.
    #[arg(long)]
    pub stock: String,
    /// The box speed.
    #[arg(long)]
    pub iso: u32,
    /// The rig profile the roll was scanned with.
    #[arg(long, value_name = "FILE")]
    pub rig: Option<PathBuf>,
}

/// Arguments for `negg lock`.
#[derive(Debug, Args)]
pub struct LockArgs {
    /// The converted frame (its sidecar is updated).
    pub frame: PathBuf,
    /// Lock the film base (Dmin).
    #[arg(long)]
    pub dmin: bool,
    /// Lock the gamma.
    #[arg(long)]
    pub gamma: bool,
    /// Lock the cast correction.
    #[arg(long)]
    pub cast: bool,
}

/// Arguments for `negg eval`.
#[derive(Debug, Args)]
pub struct EvalArgs {
    /// Corpus directory containing `manifest.toml`.
    #[arg(long, default_value = "corpus")]
    pub corpus: PathBuf,
    /// Where to write the results JSON.
    #[arg(long, default_value = "eval-results.json")]
    pub out: PathBuf,
    /// Write the JSON but do not print the table. The count of frames and
    /// reference renders skipped for missing files is still printed when
    /// it is not zero.
    #[arg(long)]
    pub quiet: bool,
    /// A previous results JSON to compare against. Any metric that worsens
    /// by more than its tolerance makes the command exit with status 3.
    #[arg(long, value_name = "FILE")]
    pub baseline: Option<PathBuf>,
    /// Tolerances and tracked cases for the baseline comparison.
    #[arg(long, value_name = "FILE", default_value = "eval.toml")]
    pub tolerances: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_defaults() {
        let cli = Cli::try_parse_from(["negg", "eval"]).unwrap();
        let Command::Eval(args) = cli.command else {
            panic!("expected eval");
        };
        assert_eq!(args.corpus, PathBuf::from("corpus"));
        assert_eq!(args.out, PathBuf::from("eval-results.json"));
        assert!(!args.quiet);
        assert_eq!(args.baseline, None);
        assert_eq!(args.tolerances, PathBuf::from("eval.toml"));
    }

    #[test]
    fn eval_flags() {
        let cli = Cli::try_parse_from([
            "negg",
            "eval",
            "--corpus",
            "c",
            "--out",
            "o.json",
            "--quiet",
            "--baseline",
            "b.json",
            "--tolerances",
            "t.toml",
        ])
        .unwrap();
        let Command::Eval(args) = cli.command else {
            panic!("expected eval");
        };
        assert_eq!(args.corpus, PathBuf::from("c"));
        assert_eq!(args.out, PathBuf::from("o.json"));
        assert!(args.quiet);
        assert_eq!(args.baseline, Some(PathBuf::from("b.json")));
        assert_eq!(args.tolerances, PathBuf::from("t.toml"));
    }

    #[test]
    fn convert_defaults() {
        let cli = Cli::try_parse_from(["negg", "convert", "frame.dng"]).unwrap();
        let Command::Convert(args) = cli.command else {
            panic!("expected convert");
        };
        assert_eq!(args.input, PathBuf::from("frame.dng"));
        assert_eq!(args.output.out, None);
        assert_eq!(args.output.settings, None);
        assert_eq!(args.output.format, Format::Tiff16);
        assert_eq!(args.output.space, None);
        assert!(!args.output.dump_stats);
        assert!(!args.timing);
    }

    #[test]
    fn convert_flags() {
        let cli = Cli::try_parse_from([
            "negg",
            "convert",
            "frame.dng",
            "--out",
            "o",
            "--settings",
            "s.json",
            "--format",
            "jpeg",
            "--space",
            "adobe_rgb",
            "--dump-stats",
            "--timing",
        ])
        .unwrap();
        let Command::Convert(args) = cli.command else {
            panic!("expected convert");
        };
        assert_eq!(args.output.out, Some(PathBuf::from("o")));
        assert_eq!(args.output.settings, Some(PathBuf::from("s.json")));
        assert_eq!(args.output.format, Format::Jpeg);
        assert_eq!(args.output.space, Some(Space::AdobeRgb));
        assert!(args.output.dump_stats);
        assert!(args.timing);
    }

    #[test]
    fn space_names_match_the_settings_schema() {
        for (name, want) in [
            ("srgb", OutputSpace::Srgb),
            ("adobe_rgb", OutputSpace::AdobeRgb),
            ("display_p3", OutputSpace::DisplayP3),
            ("pro_photo", OutputSpace::ProPhoto),
            ("rec2020", OutputSpace::Rec2020),
        ] {
            let cli = Cli::try_parse_from(["negg", "convert", "f.dng", "--space", name]).unwrap();
            let Command::Convert(args) = cli.command else {
                panic!("expected convert");
            };
            assert_eq!(
                OutputSpace::from(args.output.space.unwrap()),
                want,
                "{name}"
            );
        }
        assert!(Cli::try_parse_from(["negg", "convert", "f.dng", "--space", "adobe-rgb"]).is_err());
        assert!(Cli::try_parse_from(["negg", "convert", "f.dng", "--format", "png"]).is_err());
        assert_eq!(Format::Tiff16.extension(), "tif");
        assert_eq!(Format::Jpeg.extension(), "jpg");
    }

    #[test]
    fn batch_takes_a_directory_and_the_shared_options() {
        let cli =
            Cli::try_parse_from(["negg", "batch", "roll", "--dump-stats", "--format", "jpeg"])
                .unwrap();
        let Command::Batch(args) = cli.command else {
            panic!("expected batch");
        };
        assert_eq!(args.dir, PathBuf::from("roll"));
        assert!(args.output.dump_stats);
        assert_eq!(args.output.format, Format::Jpeg);
        assert!(Cli::try_parse_from(["negg", "batch", "roll", "--timing"]).is_err());
        assert!(Cli::try_parse_from(["negg", "convert"]).is_err());
    }

    #[test]
    fn roll_and_profile_flags_parse() {
        let cli = Cli::try_parse_from([
            "negg",
            "batch",
            "roll",
            "--rig",
            "r.toml",
            "--film",
            "f.toml",
            "--no-roll",
        ])
        .unwrap();
        let Command::Batch(args) = cli.command else {
            panic!("expected batch");
        };
        assert_eq!(args.roll.rig, Some(PathBuf::from("r.toml")));
        assert_eq!(args.roll.film, Some(PathBuf::from("f.toml")));
        assert!(args.roll.no_roll);
        let cli = Cli::try_parse_from(["negg", "convert", "f.dng"]).unwrap();
        let Command::Convert(args) = cli.command else {
            panic!("expected convert");
        };
        assert!(!args.roll.no_roll && args.roll.rig.is_none());

        let cli = Cli::try_parse_from([
            "negg",
            "profile",
            "rig",
            "--name",
            "n",
            "--source",
            "s",
            "--out",
            "o.toml",
            "--flat",
            "flat.tif",
            "--measured-on",
            "2026-01-02",
        ])
        .unwrap();
        let Command::Profile(ProfileCommand::Rig(args)) = cli.command else {
            panic!("expected profile rig");
        };
        assert_eq!(args.flat, Some(PathBuf::from("flat.tif")));
        assert_eq!(args.provenance.measured_on.as_deref(), Some("2026-01-02"));
        // --source is required.
        assert!(
            Cli::try_parse_from(["negg", "profile", "rig", "--name", "n", "--out", "o"]).is_err()
        );

        let cli = Cli::try_parse_from([
            "negg",
            "profile",
            "film",
            "roll",
            "--patches",
            "p.csv",
            "--name",
            "n",
            "--source",
            "s",
            "--stock",
            "Stock",
            "--iso",
            "400",
            "--out",
            "o.toml",
        ])
        .unwrap();
        let Command::Profile(ProfileCommand::Film(args)) = cli.command else {
            panic!("expected profile film");
        };
        assert_eq!(args.iso, 400);
        assert_eq!(args.rig, None);
        assert!(
            Cli::try_parse_from([
                "negg",
                "profile",
                "film",
                "roll",
                "--patches",
                "p.csv",
                "--name",
                "n",
                "--stock",
                "S",
                "--iso",
                "1",
                "--out",
                "o",
            ])
            .is_err()
        );

        let cli = Cli::try_parse_from(["negg", "lock", "f.dng", "--gamma"]).unwrap();
        let Command::Lock(args) = cli.command else {
            panic!("expected lock");
        };
        assert!(args.gamma && !args.dmin && !args.cast);
    }

    #[test]
    fn crop_parses_four_fractions_and_rejects_the_rest() {
        let cli =
            Cli::try_parse_from(["negg", "batch", "roll", "--crop", "0.1,0.2,0.5,0.6"]).unwrap();
        let Command::Batch(args) = cli.command else {
            panic!("expected batch");
        };
        assert_eq!(
            args.output.crop,
            Some(CropRect {
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.6
            })
        );
        let cli = Cli::try_parse_from(["negg", "convert", "f.dng"]).unwrap();
        let Command::Convert(args) = cli.command else {
            panic!("expected convert");
        };
        assert_eq!(args.output.crop, None);
        for bad in [
            "0.5,0.5,0.6,0.1",
            "1,2,3",
            "a,b,c,d",
            "0,0,1,1,1",
            "0,0,0,1",
        ] {
            assert!(
                Cli::try_parse_from(["negg", "convert", "f.dng", "--crop", bad]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn analysis_area_parses_like_the_crop() {
        let cli = Cli::try_parse_from([
            "negg",
            "batch",
            "roll",
            "--analysis-area",
            "0.1,0.2,0.5,0.6",
        ])
        .unwrap();
        let Command::Batch(args) = cli.command else {
            panic!("expected batch");
        };
        assert_eq!(
            args.output.analysis_area,
            Some(CropRect {
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.6
            })
        );
        assert_eq!(args.output.crop, None);
        let cli = Cli::try_parse_from([
            "negg",
            "convert",
            "f.dng",
            "--crop",
            "0,0,1,1",
            "--analysis-area",
            "0.1,0.1,0.8,0.8",
        ])
        .unwrap();
        let Command::Convert(args) = cli.command else {
            panic!("expected convert");
        };
        assert!(args.output.crop.is_some() && args.output.analysis_area.is_some());
        for bad in ["0.5,0.5,0.6,0.1", "1,2,3", "a,b,c,d", "0,0,0,1"] {
            assert!(
                Cli::try_parse_from(["negg", "convert", "f.dng", "--analysis-area", bad]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn preset_flags_parse() {
        let cli = Cli::try_parse_from([
            "negg",
            "batch",
            "roll",
            "--preset",
            "Rich",
            "--preset-all-frames",
        ])
        .unwrap();
        let Command::Batch(args) = cli.command else {
            panic!("expected batch");
        };
        assert_eq!(args.output.preset.as_deref(), Some("Rich"));
        assert!(args.output.preset_all_frames);
        assert!(Cli::try_parse_from(["negg", "batch", "roll", "--preset-all-frames"]).is_err());
        let cli = Cli::try_parse_from(["negg", "preset", "list"]).unwrap();
        assert!(matches!(cli.command, Command::Preset(PresetCommand::List)));
    }

    #[test]
    fn baseline_needs_a_value() {
        assert!(Cli::try_parse_from(["negg", "eval", "--baseline"]).is_err());
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["negg", "frobnicate"]).is_err());
    }
}
