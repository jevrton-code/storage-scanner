//! Árvore virtualizada: lista achatada dos nós visíveis, renderizada com `TableBuilder`.

use std::collections::{HashMap, HashSet};

use egui::{Align, Color32, Layout, RichText, Sense, Stroke, vec2};
use egui_extras::{Column, TableBuilder};
use rust_i18n::t;
use ss_core::format::{format_count, format_filetime, format_percent, format_size};
use ss_core::metrics::compression_ratio;
use ss_core::sort::{SortKey, sort_children_by};
use ss_core::{Metric, NodeId, Tree, flags};

use super::theme;

const ROW_HEIGHT: f32 = 20.0;
const INDENT: f32 = 14.0;

pub struct TreeViewState {
    pub expanded: HashSet<NodeId>,
    pub selected: Option<NodeId>,
    pub sort: SortKey,
    pub descending: bool,
    pub metric: Metric,
    /// (nó, profundidade) dos nós visíveis, na ordem de exibição.
    flat: Vec<(NodeId, u16)>,
    dirty: bool,
    /// Filhos já ordenados por nó (válido enquanto `sort`/`descending` não mudam).
    cache: HashMap<NodeId, Vec<NodeId>>,
    scroll_to: Option<usize>,
}

impl TreeViewState {
    pub fn new(metric: Metric) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(Tree::ROOT);
        Self {
            expanded,
            selected: None,
            sort: SortKey::from_metric(metric),
            descending: true,
            metric,
            flat: Vec::new(),
            dirty: true,
            cache: HashMap::new(),
            scroll_to: None,
        }
    }

    #[allow(dead_code)] // usado pelas fases de treemap/métricas
    pub fn set_metric(&mut self, metric: Metric) {
        self.metric = metric;
        self.set_sort(SortKey::from_metric(metric), true);
    }

    fn set_sort(&mut self, key: SortKey, descending: bool) {
        self.sort = key;
        self.descending = descending;
        self.cache.clear();
        self.dirty = true;
    }

    /// Clique no cabeçalho: mesma coluna inverte; coluna nova começa pela ordem natural.
    fn click_header(&mut self, key: SortKey) {
        if self.sort == key {
            self.set_sort(key, !self.descending);
        } else {
            self.set_sort(key, key != SortKey::Name);
        }
    }

    /// Invalida o cache (após mudar a árvore).
    #[allow(dead_code)] // usado pelas fases de treemap/métricas
    pub fn invalidate(&mut self) {
        self.cache.clear();
        self.dirty = true;
    }

    pub fn toggle(&mut self, id: NodeId) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.dirty = true;
    }

    /// Expande os ancestrais de `id`, seleciona e rola até o nó.
    #[allow(dead_code)] // usado pelas fases de treemap/métricas
    pub fn reveal(&mut self, tree: &Tree, id: NodeId) {
        let mut cur = tree.parent(id);
        while let Some(p) = cur {
            self.expanded.insert(p);
            cur = tree.parent(p);
        }
        self.selected = Some(id);
        self.dirty = true;
        self.rebuild_if_dirty_now(tree);
        self.scroll_to = self.flat.iter().position(|&(n, _)| n == id);
    }

    #[allow(dead_code)] // usado pelas fases de treemap/métricas
    fn rebuild_if_dirty_now(&mut self, tree: &Tree) {
        if self.dirty {
            self.rebuild(tree);
        }
    }

    fn sorted_children(&mut self, tree: &Tree, id: NodeId) -> &Vec<NodeId> {
        let (sort, desc) = (self.sort, self.descending);
        self.cache
            .entry(id)
            .or_insert_with(|| sort_children_by(tree, id, sort, desc))
    }

    fn rebuild(&mut self, tree: &Tree) {
        self.dirty = false;
        let mut flat = Vec::with_capacity(self.flat.len().max(64));
        if tree.is_empty() {
            self.flat = flat;
            return;
        }
        // Pilha explícita (profundidade pode ser grande); filhos empilhados em ordem inversa.
        let mut stack: Vec<(NodeId, u16)> = vec![(Tree::ROOT, 0)];
        while let Some((id, depth)) = stack.pop() {
            flat.push((id, depth));
            if tree.node(id).is_dir() && self.expanded.contains(&id) {
                let kids = self.sorted_children(tree, id);
                for &c in kids.iter().rev() {
                    stack.push((c, depth + 1));
                }
            }
        }
        self.flat = flat;
    }

    /// Desenha a árvore. Devolve o nó cujo menu de contexto foi pedido, se houver.
    pub fn show(&mut self, ui: &mut egui::Ui, tree: &Tree) -> TreeEvents {
        if self.dirty {
            self.rebuild(tree);
        }
        let mut events = TreeEvents::default();
        let mut toggle: Option<NodeId> = None;
        let mut select: Option<NodeId> = None;
        let mut header_click: Option<SortKey> = None;

        let metric = self.metric;
        let total_rows = self.flat.len();
        let sel = self.selected;
        let (sort, desc) = (self.sort, self.descending);

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(340.0).at_least(140.0).clip(true))
            .column(Column::initial(120.0).at_least(80.0))
            .column(Column::initial(90.0).at_least(60.0))
            .column(Column::initial(90.0).at_least(60.0))
            .column(Column::initial(80.0).at_least(50.0))
            .column(Column::initial(70.0).at_least(50.0))
            .column(Column::initial(120.0).at_least(80.0))
            .column(Column::remainder().at_least(70.0));
        if let Some(row) = self.scroll_to.take() {
            table = table.scroll_to_row(row, Some(Align::Center));
        }

        let headers: [(SortKey, String); 8] = [
            (SortKey::Name, t!("tree.column.name").to_string()),
            (SortKey::Size, t!("tree.column.percent").to_string()),
            (SortKey::Size, t!("tree.column.size").to_string()),
            (SortKey::Allocated, t!("tree.column.allocated").to_string()),
            (SortKey::Files, t!("tree.column.files").to_string()),
            (SortKey::Dirs, t!("tree.column.dirs").to_string()),
            (SortKey::Modified, t!("tree.column.modified").to_string()),
            (
                SortKey::Compression,
                t!("tree.column.compression").to_string(),
            ),
        ];

        table
            .header(22.0, |mut header| {
                for (i, (key, label)) in headers.iter().enumerate() {
                    header.col(|ui| {
                        // A coluna "% do pai" ordena por tamanho, junto com "Tamanho"; só a
                        // segunda marca a seta, evitando duas setas ao mesmo tempo.
                        let active = *key == sort && i != 1;
                        let arrow = if !active {
                            ""
                        } else if desc {
                            " ▼"
                        } else {
                            " ▲"
                        };
                        let text = RichText::new(format!("{label}{arrow}")).strong();
                        if ui.add(egui::Button::new(text).frame(false)).clicked() {
                            header_click = Some(*key);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, total_rows, |mut row| {
                    let (id, depth) = self.flat[row.index()];
                    let node = tree.node(id);
                    row.set_selected(sel == Some(id));

                    // Nome
                    row.col(|ui| {
                        ui.add_space(f32::from(depth) * INDENT);
                        if node.is_dir() && node.child_count > 0 {
                            let open = self.expanded.contains(&id);
                            let (rect, resp) =
                                ui.allocate_exact_size(vec2(14.0, ROW_HEIGHT), Sense::click());
                            paint_arrow(ui, rect, open, resp.hovered());
                            if resp.clicked() {
                                toggle = Some(id);
                            }
                        } else {
                            ui.add_space(14.0 + ui.spacing().item_spacing.x);
                        }
                        let (icon, color) = if node.is_dir() {
                            ("📁", theme::dir_color())
                        } else {
                            ("📄", ui.visuals().text_color())
                        };
                        ui.label(RichText::new(icon).color(color));
                        ui.label(tree.name(id));
                        let mut flag_text = String::new();
                        if node.has(flags::ACCESS_ERROR) {
                            flag_text.push_str(" ⚠");
                        }
                        if node.has(flags::HARDLINK) {
                            flag_text.push_str(" 🔗");
                        }
                        if node.has(flags::REPARSE) {
                            flag_text.push_str(" ↪");
                        }
                        if !flag_text.is_empty() {
                            let r = ui.label(RichText::new(flag_text).weak());
                            let mut tip = Vec::new();
                            if node.has(flags::ACCESS_ERROR) {
                                tip.push(t!("tree.flag.access_denied").to_string());
                            }
                            if node.has(flags::HARDLINK) {
                                tip.push(t!("tree.flag.hardlink").to_string());
                            }
                            if node.has(flags::REPARSE) {
                                tip.push(t!("tree.flag.reparse").to_string());
                            }
                            r.on_hover_text(tip.join("\n"));
                        }
                    });

                    // % do pai
                    row.col(|ui| {
                        let frac = match tree.parent(id) {
                            Some(p) => {
                                let base = metric.weight(tree.node(p));
                                if base == 0 {
                                    0.0
                                } else {
                                    metric.weight(node) as f32 / base as f32
                                }
                            }
                            None => 1.0,
                        };
                        percent_bar(ui, frac);
                    });
                    row.col(|ui| {
                        ui.label(format_size(node.size));
                    });
                    row.col(|ui| {
                        ui.label(format_size(node.allocated));
                    });
                    row.col(|ui| {
                        if node.is_dir() {
                            ui.label(format_count(node.files.into()));
                        }
                    });
                    row.col(|ui| {
                        if node.is_dir() {
                            ui.label(format_count(node.dirs.into()));
                        }
                    });
                    row.col(|ui| {
                        ui.label(format_filetime(node.mtime));
                    });
                    row.col(|ui| {
                        let r = compression_ratio(node);
                        if r > 0.0 {
                            ui.label(format_percent(r));
                        }
                    });

                    let resp = row.response();
                    if resp.clicked() {
                        select = Some(id);
                    }
                    if resp.double_clicked() && node.is_dir() {
                        toggle = Some(id);
                    }
                    if resp.secondary_clicked() {
                        select = Some(id);
                        events.context_menu = Some(id);
                    }
                });
            });

        if let Some(k) = header_click {
            self.click_header(k);
        }
        if let Some(id) = toggle {
            self.toggle(id);
        }
        if let Some(id) = select {
            if self.selected != Some(id) {
                events.selected = Some(id);
            }
            self.selected = Some(id);
        }
        events
    }
}

#[derive(Default)]
pub struct TreeEvents {
    /// Seleção mudou por clique do usuário.
    pub selected: Option<NodeId>,
    /// Clique direito (para o menu de contexto, fase 6).
    pub context_menu: Option<NodeId>,
}

fn paint_arrow(ui: &egui::Ui, rect: egui::Rect, open: bool, hovered: bool) {
    let c = rect.center();
    let color = if hovered {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let pts = if open {
        vec![
            c + vec2(-4.0, -2.0),
            c + vec2(4.0, -2.0),
            c + vec2(0.0, 3.0),
        ]
    } else {
        vec![
            c + vec2(-2.0, -4.0),
            c + vec2(-2.0, 4.0),
            c + vec2(3.0, 0.0),
        ]
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// Barra colorida por percentual, com o valor sobreposto.
fn percent_bar(ui: &mut egui::Ui, fraction: f32) {
    let width = ui.available_width().max(40.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT - 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, Color32::from_black_alpha(60));
    let mut filled = rect;
    filled.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    painter.rect_filled(filled, 2.0, theme::percent_color(fraction));
    painter.text(
        rect.left_center() + vec2(4.0, 0.0),
        egui::Align2::LEFT_CENTER,
        format_percent(f64::from(fraction)),
        egui::FontId::proportional(12.0),
        Color32::WHITE,
    );
}
