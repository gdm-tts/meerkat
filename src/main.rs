// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod config;
mod git;

use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str = "\
meerkat - watch the sync state of local git repositories

USAGE:
    meerkat [LIST_FILE] [-i MINUTES]

ARGS:
    LIST_FILE        repository list, one path per line
                     (default: ./meerkat.txt; a directory means DIR/meerkat.txt)

OPTIONS:
    -i, --interval   minutes between automatic fetches, 0 = off (default: 5)
    -h, --help       print this help
";

struct Args {
    list_file: PathBuf,
    interval: Duration,
}

fn parse_args() -> Result<Args, String> {
    let mut list_file: Option<PathBuf> = None;
    let mut interval_min: f64 = 5.0;

    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            Some("-i" | "--interval") => {
                let value = it.next().ok_or("missing value for --interval")?;
                interval_min = value
                    .to_str()
                    .and_then(|v| v.parse().ok())
                    .filter(|v: &f64| v.is_finite() && *v >= 0.0)
                    .ok_or("invalid value for --interval")?;
            }
            Some(s) if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ if list_file.is_none() => list_file = Some(PathBuf::from(arg)),
            _ => return Err("too many arguments".into()),
        }
    }

    let mut list_file = list_file.unwrap_or_else(|| PathBuf::from(config::DEFAULT_FILE));
    if list_file.is_dir() {
        list_file.push(config::DEFAULT_FILE);
    }
    // Absolute, so relative repo paths keep working if the cwd changes.
    let list_file = std::path::absolute(&list_file).unwrap_or(list_file);

    Ok(Args {
        list_file,
        interval: Duration::from_secs_f64(interval_min * 60.0),
    })
}

fn main() -> eframe::Result {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("meerkat")
            .with_app_id("meerkat")
            .with_inner_size([340.0, 300.0])
            .with_min_inner_size([220.0, 90.0])
            .with_drag_and_drop(true)
            .with_icon(app::icon()),
        ..Default::default()
    };

    eframe::run_native(
        "meerkat",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::MeerkatApp::new(
                cc,
                args.list_file,
                args.interval,
            )))
        }),
    )
}
