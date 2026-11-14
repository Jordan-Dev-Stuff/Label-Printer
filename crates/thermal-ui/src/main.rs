//! thermal-rs desktop app: wiring only.
//!
//! `thermal-ui [FILE]` opens the window, optionally with a file loaded.
//! `thermal-ui --render FILE [--page N] [--out PNG] [--width MM --height MM]`
//! renders exactly what would print, without a window, for checking PDFs.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod ble;
mod content;
mod devices;
mod filter;
mod jobs;
mod pdf;
mod settings;
mod theme;

use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.first().map(String::as_str) == Some("--render") {
        render_cli(&args[1..])
    } else {
        run_window(args.into_iter().next().map(PathBuf::from))
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("thermal-ui: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_window(initial_file: Option<PathBuf>) -> Result<(), String> {
    let storage = settings::storage_location();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("thermal-rs")
            .with_inner_size([1120.0, 780.0])
            .with_min_inner_size([860.0, 560.0])
            .with_icon(egui::IconData {
                rgba: theme::icon_rgba(64),
                width: 64,
                height: 64,
            }),
        // Our own probed location: eframe disables saving silently when its
        // default folder cannot be created.
        persistence_path: storage.as_ref().map(|location| location.file.clone()),
        ..Default::default()
    };
    eframe::run_native(
        "thermal-rs",
        options,
        Box::new(move |cc| Ok(Box::new(app::ThermalApp::new(cc, initial_file, storage)))),
    )
    .map_err(|error| error.to_string())
}

/// Headless render: the same worker pipeline as the window, written to a PNG.
fn render_cli(args: &[String]) -> Result<(), String> {
    let mut file = None;
    let mut page = 0_usize;
    let mut out = PathBuf::from("label-preview.png");
    let mut width_mm = 101.6_f32;
    let mut height_mm = 152.4_f32;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = || iter.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--page" => {
                page = value()?
                    .parse::<usize>()
                    .map_err(|_| "--page wants a number from 1".to_string())?
                    .checked_sub(1)
                    .ok_or("--page starts at 1")?;
            }
            "--out" => out = PathBuf::from(value()?),
            "--width" => width_mm = value()?.parse().map_err(|_| "--width wants millimetres")?,
            "--height" => height_mm = value()?.parse().map_err(|_| "--height wants millimetres")?,
            other if file.is_none() && !other.starts_with("--") => {
                file = Some(PathBuf::from(other))
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let source = file.map_or(jobs::Source::TestCard, jobs::Source::File);

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = jobs::spawn(move || {
        let _ = done_tx.send(());
    });
    let layout = jobs::Layout {
        label: content::LabelPx {
            width: settings::mm_to_dots(width_mm, tspl_core::DEFAULT_DPI),
            height: settings::mm_to_dots(height_mm, tspl_core::DEFAULT_DPI),
        },
        options: content::PrepareOptions::default(),
    };
    worker
        .requests
        .send(jobs::JobRequest::Preview {
            generation: 1,
            source,
            page,
            layout,
        })
        .map_err(|_| "render worker stopped")?;
    let _ = done_rx.recv();

    match worker.events.recv().map_err(|_| "render worker stopped")? {
        jobs::JobEvent::Preview { frame, pages, .. } => {
            let image =
                image::GrayImage::from_raw(frame.width(), frame.height(), frame.gray().to_vec())
                    .ok_or("bad frame")?;
            image
                .save(&out)
                .map_err(|error| format!("write {}: {error}", out.display()))?;
            let black = frame.gray().iter().filter(|&&v| v == 0).count();
            println!(
                "{}x{} dots, {} black ({:.1}%), pages: {}, written to {}",
                frame.width(),
                frame.height(),
                black,
                100.0 * black as f64 / frame.gray().len() as f64,
                pages.map_or("-".to_string(), |p| p.to_string()),
                out.display()
            );
            let job_size = |skip_blank| {
                let config = tspl_core::Config {
                    skip_blank,
                    ..tspl_core::Config::by482()
                };
                tspl_core::encode(&frame, &config).map_or(0, |job| job.len())
            };
            println!(
                "job: {} bytes full frame, {} bytes compact (what the app sends)",
                job_size(false),
                job_size(true)
            );
            Ok(())
        }
        jobs::JobEvent::PreviewFailed { message, .. } => Err(message),
        other => Err(format!("unexpected {other:?}")),
    }
}
