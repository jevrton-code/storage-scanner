//! Seletor de unidades.

use rust_i18n::t;
use ss_core::format::format_size;
use ss_scan::volumes::VolumeInfo;

fn label(v: &VolumeInfo) -> String {
    let (total, free) = (format_size(v.total), format_size(v.free));
    if v.label.is_empty() {
        t!(
            "drive.label",
            root = v.root,
            fs = v.fs,
            total = total,
            free = free
        )
        .to_string()
    } else {
        t!(
            "drive.label_named",
            root = v.root,
            name = v.label,
            fs = v.fs,
            total = total,
            free = free
        )
        .to_string()
    }
}

/// Combo de unidades; devolve a raiz escolhida.
pub fn show(ui: &mut egui::Ui, volumes: &[VolumeInfo], current: &str) -> Option<String> {
    let mut picked = None;
    let selected = volumes
        .iter()
        .find(|v| {
            current.eq_ignore_ascii_case(&v.root)
                || current.eq_ignore_ascii_case(v.root.trim_end_matches('\\'))
        })
        .map_or_else(|| t!("toolbar.drive_placeholder").to_string(), label);
    egui::ComboBox::from_id_salt("drive_picker")
        .selected_text(selected)
        .width(300.0)
        .show_ui(ui, |ui| {
            for v in volumes {
                if ui.selectable_label(false, label(v)).clicked() {
                    picked = Some(v.root.clone());
                }
            }
        });
    picked
}
