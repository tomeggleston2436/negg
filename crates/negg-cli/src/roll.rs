// SPDX-License-Identifier: Apache-2.0

//! `negg convert`, `negg batch` and `negg lock` on the command line (ADR
//! 0017, R-roll, R-pool, R-lock; ADR 0018, R-paths).
//!
//! The flows and roll mode live in [`negg_session`] (Ruling 68) and are
//! re-exported here so that callers keep their paths. This module maps the
//! clap arguments to a [`negg_session::OutputRequest`] and prints the
//! flows' [`FlowEvent`]s with the `negg convert: ` and `negg batch: `
//! prefixes, exactly as the CLI printed them before the move.

use std::path::Path;

use negg_session::{FlowEvent, OutputRequest, WebOutput};
pub use negg_session::{
    LockWhich, RollAnalysis, RollContext, analyse_roll, analyse_roll_frames, decode_linear,
    load_film, load_rig, lock_overrides, pool_is_current, prepare_frame, profile_path,
    read_overrides, render_in_roll, rig_mismatch,
};
use negg_types::{PoolFrame, PoolRole, RollSettings};

use crate::cli::{BatchArgs, ConvertArgs, LockArgs, OutputArgs, RollArgs};
use crate::convert::{BatchSummary, ConvertError, Converted, format_stats, source_name};

/// The file name of `path` as a string (empty when it has none).
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Builds the roll context ([`negg_session::build_context`]) and prints
/// its warnings on stderr, prefixed with `who`.
///
/// # Errors
///
/// As [`negg_session::build_context`].
pub fn build_context(
    roll: RollSettings,
    base_dir: &Path,
    who: &str,
) -> Result<RollContext, ConvertError> {
    let (ctx, warnings) = negg_session::build_context(roll, base_dir)?;
    for warning in warnings {
        eprintln!("{who}: warning: {warning}");
    }
    Ok(ctx)
}

/// The request the flows take for these output and roll arguments. The
/// `--settings` file is passed by path and read by the flow, at the point
/// the CLI always read it. `--preset` is found here
/// ([`negg_session::find_preset`] in [`negg_session::presets_dir`]).
///
/// # Errors
///
/// [`ConvertError::Usage`] for an unknown preset name or a preset file that
/// cannot be read.
pub fn output_request(output: &OutputArgs, roll: &RollArgs) -> Result<OutputRequest, ConvertError> {
    let preset = match &output.preset {
        Some(name) => Some(negg_session::find_preset(
            name,
            negg_session::presets_dir().as_deref(),
        )?),
        None => None,
    };
    Ok(OutputRequest {
        out_dir: output.out.clone(),
        format: output.format.into(),
        settings: None,
        settings_file: output.settings.clone(),
        space: output.space.map(Into::into),
        crop: output.crop,
        analysis_area: output.analysis_area,
        rig: roll.rig.clone(),
        film: roll.film.clone(),
        no_roll: roll.no_roll,
        preset,
        preset_all_frames: output.preset_all_frames,
        web: match (output.web, output.web_only) {
            (_, true) => WebOutput::Only,
            (true, false) => WebOutput::Also,
            (false, false) => WebOutput::None,
        },
    })
}

fn role_name(role: PoolRole) -> String {
    source_name(&role)
}

/// The extra `--dump-stats` lines of a frame rendered in roll mode: its
/// roles in the three pools and where its cast came from. (A frame that
/// needs an analysis area, Ruling 133, says so in [`format_stats`]: its
/// Dmax is its own in roll mode too.)
pub fn format_roll_stats(record: Option<&PoolFrame>, converted: &Converted) -> String {
    let mut s = String::new();
    if let Some(f) = record {
        s.push_str(&format!(
            "roll roles: dmin {}, gamma {}, cast {}\n",
            role_name(f.dmin_role),
            role_name(f.gamma_role),
            role_name(f.cast_role)
        ));
    }
    s.push_str(&format!(
        "cast source: {}\n",
        source_name(&converted.render.estimates.cast_source)
    ));
    s
}

/// Prints the events that read the same in `negg convert` and
/// `negg batch`; returns the others.
fn print_common(who: &str, event: FlowEvent) -> Option<FlowEvent> {
    match event {
        FlowEvent::Warning(text) => eprintln!("{who}: warning: {text}"),
        FlowEvent::UsingRoll(path) => {
            eprintln!(
                "{who}: using the roll settings and pool of {}",
                path.display()
            );
        }
        FlowEvent::FrameFailed { input, error } => {
            eprintln!("{who}: {}: {error}", input.display());
        }
        FlowEvent::WroteRoll(path) => println!("wrote {}", path.display()),
        // The decoder notes always carried the `negg convert` prefix, in
        // `negg batch` too.
        FlowEvent::Note { input, note } => {
            eprintln!("negg convert: {}: {note}", file_name(&input));
        }
        FlowEvent::PoolRejected { frame, pool } => {
            eprintln!("{who}: {frame}: rejected from the {pool} pool");
        }
        FlowEvent::NeedsAnalysisArea { frame } => {
            eprintln!(
                "{who}: warning: {frame}: Dmax is at the density cap (the film holder or \
                 another opaque area is in the analysed area), so the frame is left out of the \
                 gamma and cast pools; set an analysis area on the film, covering its clear edge \
                 and leaving out only the holder (--analysis-area sets the roll's; a frame whose \
                 sidecar has its own geometry keeps that, so give it the area there, for \
                 example in the app)"
            );
        }
        other @ FlowEvent::Converted { .. } => return Some(other),
    }
    None
}

/// Prints `wrote <output> and <sidecar>` (then `wrote <web copy>` with
/// `--web`) and, with `--dump-stats`, the stats block and (when
/// `roll_stats`) the roll lines.
fn print_converted(
    input: &Path,
    converted: &Converted,
    record: Option<&PoolFrame>,
    dump_stats: bool,
    roll_stats: bool,
) {
    println!(
        "wrote {} and {}",
        converted.output.display(),
        converted.sidecar.display()
    );
    if let Some(web) = &converted.web
        && *web != converted.output
    {
        println!("wrote {}", web.display());
    }
    if dump_stats {
        print!("{}", format_stats(input, converted));
        if roll_stats {
            print!("{}", format_roll_stats(record, converted));
        }
    }
}

/// Runs `negg convert` ([`negg_session::convert_one`]) and prints what it
/// reports. The roll lines of `--dump-stats` are printed when the roll's
/// pool was used.
///
/// # Errors
///
/// As [`negg_session::convert_one`].
pub fn run_convert(args: &ConvertArgs) -> Result<(), ConvertError> {
    let who = "negg convert";
    let req = output_request(&args.output, &args.roll)?;
    let mut pool_used = false;
    negg_session::convert_one(&args.input, &req, &mut |event| {
        if matches!(event, FlowEvent::UsingRoll(_)) {
            pool_used = true;
        }
        if let Some(FlowEvent::Converted {
            input,
            converted,
            pool_record,
        }) = print_common(who, event)
        {
            print_converted(
                &input,
                &converted,
                pool_record.as_ref(),
                args.output.dump_stats,
                pool_used,
            );
            if args.timing {
                eprintln!("{who}: timing: {}", converted.timing);
            }
        }
    })
}

/// Runs `negg batch` ([`negg_session::batch_dir`]), prints what it
/// reports and then the summary. A failing frame prints its error to
/// stderr and the batch continues.
///
/// # Errors
///
/// As [`negg_session::batch_dir`]; per-frame failures are counted in the
/// returned summary, not returned.
pub fn run_batch(args: &BatchArgs) -> Result<BatchSummary, ConvertError> {
    let who = "negg batch";
    let req = output_request(&args.output, &args.roll)?;
    let summary = negg_session::batch_dir(&args.dir, &req, &mut |event| {
        if let Some(FlowEvent::Converted {
            input,
            converted,
            pool_record,
        }) = print_common(who, event)
        {
            print_converted(
                &input,
                &converted,
                pool_record.as_ref(),
                args.output.dump_stats,
                !args.roll.no_roll,
            );
        }
    })?;
    println!("{summary}");
    Ok(summary)
}

/// Runs `negg lock` ([`negg_session::lock_frame`]): `--dmin`, `--gamma` and
/// `--cast` select; none selects all.
///
/// # Errors
///
/// As [`negg_session::lock_frame`].
pub fn run_lock(args: &LockArgs) -> Result<(), ConvertError> {
    let which = LockWhich {
        dmin: args.dmin,
        gamma: args.gamma,
        cast: args.cast,
    };
    let done = negg_session::lock_frame(&args.frame, which)?;
    println!(
        "locked {} in {}",
        done.locked.join(", "),
        done.sidecar.display()
    );
    if done.dmax_pinned {
        eprintln!(
            "negg lock: warning: {}: Dmax is at the density cap (the film holder or another \
             opaque area is in the analysed area), so the locked values come from that analysis \
             and a locked gamma keeps the frame out of the gamma pool; set an analysis area on \
             the film, convert again and lock again",
            file_name(&args.frame)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn arguments_map_to_the_request() {
        let output = OutputArgs {
            out: Some(PathBuf::from("out")),
            settings: Some(PathBuf::from("s.json")),
            format: crate::cli::Format::Jpeg,
            space: Some(crate::cli::Space::Rec2020),
            crop: None,
            analysis_area: None,
            dump_stats: true,
            preset: None,
            preset_all_frames: false,
            web: true,
            web_only: false,
        };
        let roll = RollArgs {
            rig: Some(PathBuf::from("rig.negg.toml")),
            film: None,
            no_roll: true,
        };
        let req = output_request(&output, &roll).unwrap();
        assert_eq!(req.out_dir.as_deref(), Some(Path::new("out")));
        assert_eq!(req.format, negg_session::Format::Jpeg);
        assert!(req.settings.is_none());
        assert_eq!(req.settings_file.as_deref(), Some(Path::new("s.json")));
        assert_eq!(req.space, Some(negg_types::settings::OutputSpace::Rec2020));
        assert_eq!(req.rig.as_deref(), Some(Path::new("rig.negg.toml")));
        assert!(req.no_roll);
        assert!(req.preset.is_none() && !req.preset_all_frames);
        assert_eq!(req.web, WebOutput::Also);
        let only = OutputArgs {
            web: false,
            web_only: true,
            ..output
        };
        assert_eq!(output_request(&only, &roll).unwrap().web, WebOutput::Only);
        assert_eq!(file_name(Path::new("r/f.dng")), "f.dng");
    }
}
