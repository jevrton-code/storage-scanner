//! Cores e estilo.

use egui::Color32;

pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
}

/// Gradiente da barra de percentual: azul-esverdeado (pouco) → amarelo → vermelho (muito).
pub fn percent_color(fraction: f32) -> Color32 {
    let f = fraction.clamp(0.0, 1.0);
    let hue = (1.0 - f) * 0.45; // 0.45 (verde-água) → 0.0 (vermelho)
    egui::ecolor::Hsva::new(hue, 0.65, 0.75, 1.0).into()
}

pub fn dir_color() -> Color32 {
    Color32::from_rgb(230, 190, 90)
}
