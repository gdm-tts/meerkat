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
    meerkat [LIST_FILE] [-i MINUTES] [-f]

ARGS:
    LIST_FILE        repository list, one path per line
                     (default: ./meerkat.txt; a directory means DIR/meerkat.txt)

OPTIONS:
    -i, --interval   minutes between automatic fetches, 0 = off (default: 5)
    -f, --foreground stay attached to the terminal it was started from
                     (by default it detaches and gives the terminal back)
    -h, --help       print this help
";

struct Args {
    list_file: PathBuf,
    interval: Duration,
    foreground: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut list_file: Option<PathBuf> = None;
    let mut interval_min: f64 = 5.0;
    let mut foreground = false;

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
            Some("-f" | "--foreground") => foreground = true,
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
        foreground,
    })
}

/// When started from a terminal, re-launch in the background in a new session
/// and exit, so the terminal is released (or closes, if a file manager opened
/// it just to run us). Windows gets the same effect from `windows_subsystem`.
#[cfg(unix)]
fn detach_from_terminal() {
    use std::io::IsTerminal;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    unsafe extern "C" {
        fn setsid() -> i32;
    }

    if !std::io::stdin().is_terminal() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.args(std::env::args_os().skip(1))
        .arg("--foreground")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe; leaving the terminal's session means
    // closing the terminal (SIGHUP) no longer takes the window down with it.
    unsafe {
        cmd.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    // If spawning fails, just keep running attached to the terminal.
    if cmd.spawn().is_ok() {
        std::process::exit(0);
    }
}

fn main() -> eframe::Result {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    if !args.foreground {
        #[cfg(unix)]
        detach_from_terminal();
    }

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
