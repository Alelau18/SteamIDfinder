#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod history;
mod profile;
mod steamid;
#[cfg(test)]
mod test_util;
mod worker;

use std::sync::Arc;

use eframe::egui;

const HELP: &str = "\
SteamIDfinder: turn any Steam ID into a Steam profile link.

Usage: steamidfinder [ID ...]

Each ID can be a SteamID64, SteamID2 (STEAM_0:0:11101), SteamID3 ([U:1:22202]),
account ID, steamcommunity.com profile URL or custom URL name. IDs given on the
command line are looked up as soon as the window opens.

Options:
  -h, --help     Show this help
  -V, --version  Show the version";

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .filter(|a| a != "--")
        .collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return Ok(());
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("steamidfinder {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let initial_query = args.join(" ");

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("SteamIDfinder")
        .with_app_id("steamidfinder")
        .with_inner_size([680.0, 760.0])
        .with_min_inner_size([420.0, 360.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "SteamIDfinder",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, &initial_query)))),
    )
}
