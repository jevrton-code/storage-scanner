//! Barra de status.

use rust_i18n::t;
use ss_core::Tree;
use ss_core::format::{format_count, format_size};
use ss_scan::Backend;

use crate::state::ScanResult;

pub fn show(
    ui: &mut egui::Ui,
    result: Option<&ScanResult>,
    selected: Option<u32>,
    notice: Option<&str>,
) {
    ui.horizontal(|ui| {
        if let Some(n) = notice {
            ui.label(n);
            ui.separator();
        }
        match result {
            None => {
                ui.label(t!("status.idle"));
            }
            Some(r) => {
                let root = r.tree.node(Tree::ROOT);
                let backend = match r.backend {
                    Backend::Mft => t!("backend.mft"),
                    _ => t!("backend.walk"),
                };
                ui.label(
                    t!(
                        "status.done",
                        backend = backend,
                        files = format_count(root.files.into()),
                        dirs = format_count(root.dirs.into()),
                        size = format_size(root.size),
                        allocated = format_size(root.allocated),
                        secs = format!("{:.2}", r.elapsed.as_secs_f64()).replace('.', ",")
                    )
                    .to_string(),
                );
                if r.errors > 0 {
                    ui.separator();
                    ui.label(t!("status.denied", n = format_count(r.errors)).to_string());
                }
                if let Some(id) = selected {
                    ui.separator();
                    ui.label(
                        t!(
                            "status.selected",
                            path = r.tree.path(id),
                            size = format_size(r.tree.node(id).size)
                        )
                        .to_string(),
                    );
                }
            }
        }
    });
}
