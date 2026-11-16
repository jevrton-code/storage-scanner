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
    /// Alvo escolhido na lista de recentes.
    pub picked_recent: Option<String>,
    pub export_csv: bool,
    pub browse: bool,
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

/// Dados somente leitura exibidos na barra.
pub struct ToolbarInput<'a> {
    pub volumes: &'a [VolumeInfo],
    pub devices: &'a [MtpDevice],
    pub scanning: bool,
    pub recent: &'a [String],
    pub can_export: bool,
}

pub fn show(
    ui: &mut egui::Ui,
    path: &mut String,
    metric: &mut Metric,
    input: &ToolbarInput<'_>,
) -> ToolbarOutput {
    let ToolbarInput {
        volumes,
        devices,
        scanning,
        recent,
        can_export,
    } = *input;
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
            if ui.button(t!("toolbar.browse")).clicked() {
                out.browse = true;
            }
            ui.menu_button("🕘", |ui| {
                if recent.is_empty() {
                    ui.weak(t!("toolbar.no_recent").to_string());
                }
                for r in recent {
                    if ui.button(r).clicked() {
                        out.picked_recent = Some(r.clone());
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text(t!("toolbar.recent"));
            let edit = ui.add(
                egui::TextEdit::singleline(path)
                    .hint_text(t!("toolbar.path_hint"))
                    .desired_width((ui.available_width() - 330.0).max(120.0)),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                out.analyze = true;
            }
        });
        if ui
            .add_enabled(can_export, egui::Button::new(t!("toolbar.export_short")))
            .on_hover_text(t!("toolbar.export_csv"))
            .clicked()
        {
            out.export_csv = true;
        }
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
