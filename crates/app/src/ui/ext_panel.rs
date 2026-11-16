//! Painel de tipos de arquivo (por extensão).

use egui::{Align, Layout, RichText, Sense, vec2};
use egui_extras::{Column, TableBuilder};
use rust_i18n::t;
use ss_core::ext_stats::{ExtStat, sort_stats};
use ss_core::format::{format_count, format_percent, format_size};
use ss_core::{Metric, Tree};

use super::treemap_view::ext_color;

const ROW_HEIGHT: f32 = 20.0;

#[derive(Default)]
pub struct ExtPanelState {
    /// Métrica pela qual a lista já foi ordenada.
    sorted_by: Option<Metric>,
    list: Vec<ExtStat>,
    /// Origem da lista (identifica o resultado atual).
    source_gen: Option<(usize, u64)>,
}

impl ExtPanelState {
    /// Desenha o painel; `highlight` é a extensão selecionada (clique alterna).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tree: &Tree,
        stats: &[ExtStat],
        generation: u64,
        metric: Metric,
        highlight: &mut Option<String>,
    ) {
        // Reordena só quando a métrica ou o conjunto de dados muda.
        let source = (stats.len(), generation);
        if self.source_gen != Some(source) || self.sorted_by != Some(metric) {
            self.list = stats.to_vec();
            sort_stats(&mut self.list, metric);
            self.sorted_by = Some(metric);
            self.source_gen = Some(source);
        }
        let root = tree.node(Tree::ROOT);
        let total_key = match metric {
            Metric::Size | Metric::Modified => root.size,
            Metric::Allocated => root.allocated,
            Metric::FileCount => u64::from(root.files),
            Metric::Compression => root.size.saturating_sub(root.allocated),
        }
        .max(1);

        ui.horizontal(|ui| {
            ui.strong(t!("ext_panel.title"));
            if highlight.is_some() && ui.small_button(t!("ext_panel.clear")).clicked() {
                *highlight = None;
            }
        });
        if highlight.is_none() {
            ui.weak(t!("ext_panel.hint"));
        }

        let mut clicked: Option<String> = None;
        TableBuilder::new(ui)
            .striped(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(110.0).at_least(60.0).clip(true))
            .column(Column::initial(55.0).at_least(40.0))
            .column(Column::initial(75.0).at_least(50.0))
            .column(Column::initial(75.0).at_least(50.0))
            .column(Column::remainder().at_least(50.0))
            .header(20.0, |mut h| {
                let titles = [
                    t!("ext_panel.type"),
                    t!("ext_panel.percent"),
                    t!("ext_panel.size"),
                    t!("ext_panel.allocated"),
                    t!("ext_panel.count"),
                ];
                for title in titles {
                    h.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.list.len(), |mut row| {
                    let s = &self.list[row.index()];
                    row.set_selected(highlight.as_deref() == Some(s.ext.as_str()));
                    row.col(|ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, ext_color(&s.ext));
                        if s.ext.is_empty() {
                            ui.label(RichText::new(t!("ext_panel.none")).italics());
                        } else {
                            ui.label(format!(".{}", s.ext));
                        }
                    });
                    row.col(|ui| {
                        ui.label(format_percent(s.key(metric) as f64 / total_key as f64));
                    });
                    row.col(|ui| {
                        ui.label(format_size(s.size));
                    });
                    row.col(|ui| {
                        ui.label(format_size(s.allocated));
                    });
                    row.col(|ui| {
                        ui.label(format_count(s.count));
                    });
                    if row.response().clicked() {
                        clicked = Some(s.ext.clone());
                    }
                });
            });
        if let Some(ext) = clicked {
            *highlight = if highlight.as_deref() == Some(ext.as_str()) {
                None
            } else {
                Some(ext)
            };
        }
    }
}
