#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

rust_i18n::i18n!("locales", fallback = "pt-BR");

mod app;
mod cli;
mod state;
mod ui;

use rust_i18n::t;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(parsed) = cli::parse(&args) {
        match parsed {
            Ok(a) => std::process::exit(cli::run(a)),
            Err(msg) => {
                eprintln!("{msg}");
                std::process::exit(2);
            }
        }
    }
    // Um argumento posicional (ex.: do menu do Explorer) é o alvo inicial.
    let initial_target = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map(std::path::PathBuf::from);
    rust_i18n::set_locale("pt-BR");
    tracing_subscriber::fmt::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(t!("app.title").to_string())
            .with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        &t!("app.title"),
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial_target)))),
    )
}
