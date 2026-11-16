//! Treemap interativo: layout em thread de fundo, desenho em uma única `Mesh`.

use std::sync::Arc;

use crossbeam_channel::Receiver;
use egui::{Color32, Pos2, Rect, Sense, Stroke, vec2};
use rust_i18n::t;
use ss_core::ext_stats::extension_of;
use ss_core::format::{format_count, format_size};
use ss_core::metrics::compression_ratio;
use ss_core::treemap::{self, Params, TmRect};
use ss_core::{Metric, NodeId, Tree};

use super::theme;

#[derive(Clone, Copy, PartialEq)]
struct Key {
    zoom: NodeId,
    metric: Metric,
    w: u32,
    h: u32,
    generation: u64,
}

struct Layout {
    key: Key,
    id: u64,
    rects: Arc<Vec<TmRect>>,
}

struct MeshCache {
    layout_id: u64,
    area: Rect,
    metric: Metric,
    highlight: Option<String>,
    mesh: Arc<egui::Mesh>,
}

pub struct TreemapState {
    /// Pasta exibida (raiz do zoom).
    pub zoom: NodeId,
    layout: Option<Layout>,
    pending: Option<(Key, Receiver<Arc<Vec<TmRect>>>)>,
    next_id: u64,
    mesh: Option<MeshCache>,
}

#[derive(Default)]
pub struct TreemapEvents {
    pub select: Option<NodeId>,
}

impl Default for TreemapState {
    fn default() -> Self {
        Self::new()
    }
}

impl TreemapState {
    pub fn new() -> Self {
        Self {
            zoom: Tree::ROOT,
            layout: None,
            pending: None,
            next_id: 1,
            mesh: None,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tree: &Arc<Tree>,
        generation: u64,
        metric: Metric,
        selected: Option<NodeId>,
        highlight_ext: Option<&str>,
    ) -> TreemapEvents {
        let mut events = TreemapEvents::default();
        if self.zoom as usize >= tree.len() {
            self.zoom = Tree::ROOT;
        }
        self.breadcrumb(ui, tree);

        let (area, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
        if area.width() < 8.0 || area.height() < 8.0 {
            return events;
        }
        let key = Key {
            zoom: self.zoom,
            metric,
            // Arredonda para evitar recalcular a cada pixel durante o redimensionamento.
            w: (area.width() / 8.0).round() as u32 * 8,
            h: (area.height() / 8.0).round() as u32 * 8,
            generation,
        };
        self.poll_layout(ui.ctx());
        self.request_layout(ui.ctx(), tree, key);

        let Some(layout) = &self.layout else {
            ui.painter()
                .rect_filled(area, 0.0, ui.visuals().extreme_bg_color);
            ui.put(area, egui::Spinner::new());
            return events;
        };
        let rects = layout.rects.clone();
        let layout_id = layout.id;
        let (lw, lh) = (layout.key.w as f32, layout.key.h as f32);
        let (sx, sy) = (area.width() / lw, area.height() / lh);

        // Malha única, refeita só quando layout/posição/tamanho mudam.
        let rebuild = self.mesh.as_ref().is_none_or(|m| {
            m.layout_id != layout_id
                || m.area != area
                || m.metric != metric
                || m.highlight.as_deref() != highlight_ext
        });
        if rebuild {
            let mesh = build_mesh(tree, &rects, area, (sx, sy), metric, highlight_ext);
            self.mesh = Some(MeshCache {
                layout_id,
                area,
                metric,
                highlight: highlight_ext.map(str::to_owned),
                mesh: Arc::new(mesh),
            });
        }
        if let Some(m) = &self.mesh {
            ui.painter().add(egui::Shape::mesh(m.mesh.clone()));
        }

        let to_screen = |r: &TmRect| {
            Rect::from_min_size(
                Pos2::new(area.min.x + r.x * sx, area.min.y + r.y * sy),
                vec2(r.w * sx, r.h * sy),
            )
        };

        // Seleção.
        if let Some(sel) = selected
            && let Some(r) = rects.iter().find(|r| r.node == sel)
        {
            ui.painter().rect_stroke(
                to_screen(r),
                0.0,
                Stroke::new(2.0, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }

        // Hover: o último retângulo que contém o ponteiro é o mais profundo.
        let hovered = resp.hover_pos().and_then(|p| {
            let (lx, ly) = ((p.x - area.min.x) / sx, (p.y - area.min.y) / sy);
            rects.iter().rev().find(|r| r.contains(lx, ly))
        });
        if let Some(r) = hovered {
            ui.painter().rect_stroke(
                to_screen(r),
                0.0,
                Stroke::new(1.5, Color32::from_rgb(255, 230, 120)),
                egui::StrokeKind::Inside,
            );
            let node = tree.node(r.node);
            let path = tree.path(r.node);
            let size = format_size(node.size);
            let files = node.is_dir().then(|| format_count(node.files.into()));
            resp.clone().on_hover_ui_at_pointer(|ui| {
                ui.label(path);
                ui.label(size);
                if let Some(f) = files {
                    ui.label(t!("treemap.tooltip_files", n = f).to_string());
                }
            });
        }

        if let Some(r) = hovered {
            if resp.double_clicked() {
                let node = tree.node(r.node);
                self.zoom = if node.is_dir() {
                    r.node
                } else {
                    tree.parent(r.node).unwrap_or(Tree::ROOT)
                };
                events.select = Some(r.node);
            } else if resp.clicked() {
                events.select = Some(r.node);
            }
        }
        events
    }

    fn breadcrumb(&mut self, ui: &mut egui::Ui, tree: &Tree) {
        let mut chain = vec![self.zoom];
        while let Some(p) = tree.parent(*chain.last().expect("não vazio")) {
            chain.push(p);
        }
        chain.reverse();
        ui.horizontal_wrapped(|ui| {
            for (i, &id) in chain.iter().enumerate() {
                if i > 0 {
                    ui.label("›");
                }
                let label = tree.name(id);
                if i + 1 == chain.len() {
                    ui.strong(label);
                } else if ui.link(label).clicked() {
                    self.zoom = id;
                }
            }
        });
    }

    fn poll_layout(&mut self, ctx: &egui::Context) {
        let done = match &self.pending {
            Some((key, rx)) => rx.try_recv().ok().map(|rects| (*key, rects)),
            None => None,
        };
        if let Some((key, rects)) = done {
            self.pending = None;
            self.layout = Some(Layout {
                key,
                id: self.next_id,
                rects,
            });
            self.next_id += 1;
            self.mesh = None;
            ctx.request_repaint();
        }
    }

    fn request_layout(&mut self, ctx: &egui::Context, tree: &Arc<Tree>, key: Key) {
        if self.layout.as_ref().is_some_and(|l| l.key == key) {
            return;
        }
        if self.pending.is_some() {
            return; // ao terminar, o próximo quadro pede o layout atual
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (tree, ctx) = (tree.clone(), ctx.clone());
        std::thread::spawn(move || {
            let rects = treemap::layout(
                &tree,
                key.zoom,
                key.metric,
                key.w as f32,
                key.h as f32,
                Params::default(),
            );
            let _ = tx.send(Arc::new(rects));
            ctx.request_repaint();
        });
        self.pending = Some((key, rx));
    }
}

/// Cor estável por extensão (matiz a partir de um hash).
pub fn ext_color(ext: &str) -> Color32 {
    if ext.is_empty() {
        return Color32::from_rgb(130, 130, 140);
    }
    let mut h: u32 = 2166136261;
    for b in ext.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.55, 0.78, 1.0).into()
}

fn build_mesh(
    tree: &Tree,
    rects: &[TmRect],
    area: Rect,
    (sx, sy): (f32, f32),
    metric: Metric,
    highlight: Option<&str>,
) -> egui::Mesh {
    let now = theme::now_filetime();
    let mut mesh = egui::Mesh::default();
    mesh.reserve_triangles(rects.len() * 2);
    mesh.reserve_vertices(rects.len() * 4);
    for r in rects {
        let rect = Rect::from_min_size(
            Pos2::new(area.min.x + r.x * sx, area.min.y + r.y * sy),
            vec2(r.w * sx, r.h * sy),
        );
        let shade = 1.0 - 0.04 * f32::from(r.depth.min(8));
        let color = if !r.leaf {
            // Fundo da pasta: tom neutro que clareia levemente por profundidade.
            let v = (38.0 + 10.0 * f32::from(r.depth.min(6))) as u8;
            Color32::from_rgb(v, v, v + 4)
        } else {
            let node = tree.node(r.node);
            let mut dimmed = false;
            let base = if node.is_dir() {
                dimmed = highlight.is_some();
                theme::dir_color()
            } else {
                match metric {
                    Metric::Modified => theme::age_color(node.mtime, now),
                    Metric::Compression => theme::compression_color(compression_ratio(node) as f32),
                    _ => {
                        let ext = extension_of(tree.name(r.node));
                        dimmed = highlight.is_some_and(|h| h != ext);
                        ext_color(&ext)
                    }
                }
            };
            let [cr, cg, cb, _] = base.to_array();
            let k = if dimmed { shade * 0.28 } else { shade };
            Color32::from_rgb(
                (f32::from(cr) * k) as u8,
                (f32::from(cg) * k) as u8,
                (f32::from(cb) * k) as u8,
            )
        };
        // Folgas de 0,5 px dão a grade visual entre blocos.
        let draw = if r.leaf && rect.width() > 3.0 && rect.height() > 3.0 {
            rect.shrink(0.5)
        } else {
            rect
        };
        mesh.add_colored_rect(draw, color);
    }
    mesh
}
