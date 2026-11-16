//! Barra de ferramentas: unidade/pasta, Analisar, Parar.

use rust_i18n::t;
use ss_scan::volumes::VolumeInfo;

use super::drive_picker;

#[derive(Default)]
pub struct ToolbarOutput {
    pub analyze: bool,
    pub stop: bool,
    pub refresh_drives: bool,
}

pub fn show(
    ui: &mut egui::Ui,
    path: &mut String,
    volumes: &[VolumeInfo],
    scanning: bool,
) -> ToolbarOutput {
    let mut out = ToolbarOutput::default();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!scanning, |ui| {
            if let Some(root) = drive_picker::show(ui, volumes, path) {
                *path = root;
                out.analyze = true;
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
                    .desired_width(ui.available_width() - 190.0),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                out.analyze = true;
            }
        });
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
