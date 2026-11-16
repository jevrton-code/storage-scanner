//! Cores e estilo.

use egui::Color32;

pub fn apply(ctx: &egui::Context, choice: crate::config::ThemeChoice) {
    ctx.set_theme(choice.preference());
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

/// Cor pela idade: recente = verde-claro; antigo = azul-acinzentado escuro (escala logarítmica).
pub fn age_color(mtime: i64, now: i64) -> Color32 {
    const DAY: f64 = 864_000_000_000.0; // 100 ns por dia
    if mtime <= 0 {
        return Color32::from_rgb(90, 90, 100);
    }
    let days = ((now - mtime).max(0) as f64 / DAY).max(0.0);
    // 0 dia -> 0.0 ; ~4 anos -> 1.0
    let t = ((days + 1.0).ln() / (1500.0f64).ln()).clamp(0.0, 1.0) as f32;
    let hue = 0.33 + t * (0.62 - 0.33); // verde -> azul
    egui::ecolor::Hsva::new(hue, 0.6 - 0.3 * t, 0.85 - 0.4 * t, 1.0).into()
}

/// Cor pela taxa de compressão (0 = nenhuma -> cinza; 1 = máxima -> ciano forte).
pub fn compression_color(ratio: f32) -> Color32 {
    let r = ratio.clamp(0.0, 1.0);
    if r <= 0.0 {
        return Color32::from_rgb(95, 95, 105);
    }
    egui::ecolor::Hsva::new(0.55 - 0.1 * r, 0.3 + 0.6 * r, 0.7, 1.0).into()
}

/// FILETIME atual.
pub fn now_filetime() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    (secs + 11_644_473_600) * 10_000_000
}
