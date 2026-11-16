//! Barra de ferramentas: unidade/pasta, Analisar, Parar.

use rust_i18n::t;
use ss_core::Metric;
use ss_scan::mtp::MtpDevice;
use ss_scan::volumes::VolumeInfo;

use super::drive_picker::{self, Pick};

#[derive(Default)]
pub struct ToolbarOutput {
    pub analyze: bool,
    pub stop: bool,
    pub refresh_drives: bool,
    pub open_settings: bool,
    /// Dispositivo MTP escolhido no seletor.
    pub picked_device: Option<MtpDevice>,
}

pub fn metric_label(m: Metric) -> String {
    match m {
        Metric::Size => t!("metric.size"),
        Metric::Allocated => t!("metric.allocated"),
        Metric::FileCount => t!("metric.files"),
        Metric::Modified => t!("metric.modified"),
        Metric::Compression => t!("metric.compression"),
    }
    .to_string()
}

pub fn show(
    ui: &mut egui::Ui,
    path: &mut String,
    volumes: &[VolumeInfo],
    devices: &[MtpDevice],
    scanning: bool,
    metric: &mut Metric,
) -> ToolbarOutput {
    let mut out = ToolbarOutput::default();
    ui.horizontal(|ui| {
        ui.label(t!("toolbar.metric"));
        egui::ComboBox::from_id_salt("metric_picker")
            .selected_text(metric_label(*metric))
            .show_ui(ui, |ui| {
                for m in Metric::ALL {
                    if ui.selectable_label(*metric == m, metric_label(m)).clicked() {
                        *metric = m;
                    }
                }
            });
        ui.separator();
        ui.add_enabled_ui(!scanning, |ui| {
            match drive_picker::show(ui, volumes, devices, path) {
                Some(Pick::Volume(root)) => {
                    *path = root;
                    out.analyze = true;
                }
                Some(Pick::Device(dev)) => {
                    *path = drive_picker::device_label(&dev);
                    out.picked_device = Some(dev);
                    out.analyze = true;
                }
                None => {}
            }
            if ui
                .small_button("⟳")
                .on_hover_text(t!("toolbar.refresh_drives"))
                .clicked()
            {
                out.refresh_drives = true;
            }
            if ui.button(t!("toolbar.browse")).clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                *path = dir.to_string_lossy().into_owned();
                out.analyze = true;
            }
            let edit = ui.add(
                egui::TextEdit::singleline(path)
                    .hint_text(t!("toolbar.path_hint"))
                    .desired_width((ui.available_width() - 230.0).max(120.0)),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                out.analyze = true;
            }
        });
        if ui
            .button("⚙")
            .on_hover_text(t!("toolbar.settings"))
            .clicked()
        {
            out.open_settings = true;
        }
        if scanning {
            if ui.button(t!("toolbar.stop")).clicked() {
                out.stop = true;
            }
        } else if ui
            .add_enabled(
                !path.trim().is_empty(),
                egui::Button::new(t!("toolbar.analyze")),
            )
            .clicked()
        {
            out.analyze = true;
        }
    });
    out
}
