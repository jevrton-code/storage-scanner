//! Janelas de aviso/erro simples.

pub struct ErrorDialog {
    pub message: String,
}

/// Mostra o erro; devolve `true` quando o usuário o dispensa.
pub fn show_error(ctx: &egui::Context, err: &ErrorDialog) -> bool {
    let mut open = true;
    let mut close = false;
    egui::Window::new("Storage Scanner")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(&err.message);
            ui.add_space(8.0);
            if ui.button("OK").clicked() {
                close = true;
            }
        });
    !open || close
}

/// Alterações feitas na janela de configurações.
#[derive(Default)]
pub struct SettingsChange {
    pub config: bool,
    pub explorer: bool,
}

/// Janela de configurações; edita `cfg` e `explorer` diretamente e informa o que mudou.
pub fn show_settings(
    ctx: &egui::Context,
    open: &mut bool,
    cfg: &mut crate::config::Config,
    explorer: &mut bool,
) -> SettingsChange {
    use crate::config::{ThemeChoice, metric_from_str, metric_to_str};
    use crate::ui::toolbar::metric_label;
    use rust_i18n::t;
    let mut ch = SettingsChange::default();
    egui::Window::new(t!("settings.title").to_string())
        .collapsible(false)
        .resizable(false)
        .open(open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Grid::new("settings_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label(t!("settings.theme").to_string());
                    ui.horizontal(|ui| {
                        for (v, key) in [
                            (ThemeChoice::Dark, "settings.theme_dark"),
                            (ThemeChoice::Light, "settings.theme_light"),
                            (ThemeChoice::System, "settings.theme_system"),
                        ] {
                            if ui
                                .selectable_label(cfg.theme == v, t!(key).to_string())
                                .clicked()
                            {
                                cfg.theme = v;
                                ch.config = true;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label(t!("settings.default_metric").to_string());
                    let cur = metric_from_str(&cfg.metric);
                    egui::ComboBox::from_id_salt("default_metric")
                        .selected_text(metric_label(cur))
                        .show_ui(ui, |ui| {
                            for m in ss_core::Metric::ALL {
                                if ui.selectable_label(cur == m, metric_label(m)).clicked() {
                                    cfg.metric = metric_to_str(m).to_string();
                                    ch.config = true;
                                }
                            }
                        });
                    ui.end_row();

                    ui.label(t!("settings.columns").to_string());
                    ui.vertical(|ui| {
                        let c = &mut cfg.columns;
                        for (flag, key) in [
                            (&mut c.percent, "tree.column.percent"),
                            (&mut c.size, "tree.column.size"),
                            (&mut c.allocated, "tree.column.allocated"),
                            (&mut c.files, "tree.column.files"),
                            (&mut c.dirs, "tree.column.dirs"),
                            (&mut c.modified, "tree.column.modified"),
                            (&mut c.compression, "tree.column.compression"),
                        ] {
                            if ui.checkbox(flag, t!(key).to_string()).changed() {
                                ch.config = true;
                            }
                        }
                    });
                    ui.end_row();
                });
            ui.separator();
            if ui
                .checkbox(explorer, t!("settings.explorer_integration").to_string())
                .changed()
            {
                ch.explorer = true;
            }
            ui.weak(t!("settings.explorer_hint").to_string());
        });
    ch
}
