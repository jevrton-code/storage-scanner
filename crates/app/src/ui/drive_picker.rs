//! Seletor de unidades e dispositivos portáteis (MTP).

use rust_i18n::t;
use ss_core::format::format_size;
use ss_scan::mtp::MtpDevice;
use ss_scan::volumes::VolumeInfo;

pub enum Pick {
    Volume(String),
    Device(MtpDevice),
}

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

/// Texto exibido no campo de caminho quando um dispositivo MTP é escolhido.
pub fn device_label(d: &MtpDevice) -> String {
    t!("drive.device", name = d.name.as_str()).to_string()
}

/// Combo de unidades e dispositivos; devolve a escolha, se houve.
pub fn show(
    ui: &mut egui::Ui,
    volumes: &[VolumeInfo],
    devices: &[MtpDevice],
    current: &str,
) -> Option<Pick> {
    let mut picked = None;
    let selected = volumes
        .iter()
        .find(|v| {
            current.eq_ignore_ascii_case(&v.root)
                || current.eq_ignore_ascii_case(v.root.trim_end_matches('\\'))
        })
        .map(label)
        .or_else(|| {
            devices
                .iter()
                .find(|d| device_label(d) == current)
                .map(device_label)
        })
        .unwrap_or_else(|| t!("toolbar.drive_placeholder").to_string());
    egui::ComboBox::from_id_salt("drive_picker")
        .selected_text(selected)
        .width(300.0)
        .show_ui(ui, |ui| {
            for v in volumes {
                if ui.selectable_label(false, label(v)).clicked() {
                    picked = Some(Pick::Volume(v.root.clone()));
                }
            }
            if !devices.is_empty() {
                ui.separator();
            }
            for d in devices {
                if ui.selectable_label(false, device_label(d)).clicked() {
                    picked = Some(Pick::Device(d.clone()));
                }
            }
        });
    picked
}
