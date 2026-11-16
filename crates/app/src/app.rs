use rust_i18n::t;

#[derive(Default)]
pub struct App;

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.centered_and_justified(|ui| {
            ui.label(t!("app.empty").to_string());
        });
    }
}
