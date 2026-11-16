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
