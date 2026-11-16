//! Menu de contexto próprio do egui, diálogos de confirmação e progresso das ações.

use egui::{Area, Button, Frame, Id, Key, Order, Pos2};
use rust_i18n::t;
use ss_core::format::format_size;

use crate::actions::{ActionJob, JobProgress, MenuAction};

/// O que o menu precisa saber do item para habilitar/desabilitar entradas.
pub struct MenuInfo {
    pub path: String,
    /// O caminho existe de fato no disco (nós sintéticos como "[Órfãos]" não existem).
    pub exists: bool,
    pub protected: bool,
    pub ntfs: bool,
    pub compressed: bool,
    /// Item de um dispositivo MTP: só copiar caminho, copiar para o PC e excluir.
    pub mtp: bool,
}

pub struct MenuState {
    pub node: u32,
    pub pos: Pos2,
}

/// Desenha o menu em `state.pos`. Devolve (ação escolhida, fechar).
pub fn show(ctx: &egui::Context, state: &MenuState, info: &MenuInfo) -> (Option<MenuAction>, bool) {
    let mut chosen = None;
    let area = Area::new(Id::unique("context_menu"))
        .order(Order::Foreground)
        .fixed_pos(state.pos)
        .show(ctx, |ui| {
            Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(230.0);
                ui.add(egui::Label::new(egui::RichText::new(&info.path).weak().small()).truncate());
                ui.separator();
                let mut item = |ui: &mut egui::Ui, enabled: bool, label: String, a: MenuAction| {
                    if ui
                        .add_enabled(enabled, Button::new(label).frame(false))
                        .clicked()
                    {
                        chosen = Some(a);
                    }
                };
                if info.mtp {
                    item(
                        ui,
                        true,
                        t!("menu.copy_path").to_string(),
                        MenuAction::CopyPath,
                    );
                    item(
                        ui,
                        true,
                        t!("menu.mtp_copy_to_pc").to_string(),
                        MenuAction::CopyTo,
                    );
                    ui.separator();
                    item(
                        ui,
                        true,
                        t!("menu.delete_permanent").to_string(),
                        MenuAction::DeletePermanent,
                    );
                    ui.weak(t!("menu.mtp_limited").to_string());
                    return;
                }
                let ok = info.exists;
                let destructive = ok && !info.protected;
                item(ui, ok, t!("menu.open").to_string(), MenuAction::Open);
                item(ui, ok, t!("menu.reveal").to_string(), MenuAction::Reveal);
                item(
                    ui,
                    true,
                    t!("menu.copy_path").to_string(),
                    MenuAction::CopyPath,
                );
                ui.separator();
                item(
                    ui,
                    destructive,
                    t!("menu.recycle").to_string(),
                    MenuAction::Recycle,
                );
                item(
                    ui,
                    destructive,
                    t!("menu.delete_permanent").to_string(),
                    MenuAction::DeletePermanent,
                );
                item(
                    ui,
                    destructive,
                    t!("menu.move_to").to_string(),
                    MenuAction::MoveTo,
                );
                item(ui, ok, t!("menu.copy_to").to_string(), MenuAction::CopyTo);
                ui.separator();
                item(ui, ok, t!("menu.zip").to_string(), MenuAction::Zip);
                if info.ntfs {
                    let (label, a) = if info.compressed {
                        (t!("menu.ntfs_uncompress"), MenuAction::Compress(false))
                    } else {
                        (t!("menu.ntfs_compress"), MenuAction::Compress(true))
                    };
                    item(ui, destructive, label.to_string(), a);
                }
                ui.separator();
                item(
                    ui,
                    ok,
                    t!("menu.native").to_string(),
                    MenuAction::NativeMenu,
                );
                if info.protected && ok {
                    ui.separator();
                    ui.weak(t!("menu.protected").to_string());
                }
            });
        });
    let outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| !area.response.rect.contains(p));
    let escape = ctx.input(|i| i.key_pressed(Key::Escape));
    (chosen, chosen.is_some() || outside || escape)
}

pub struct ConfirmDelete {
    pub paths: Vec<std::path::PathBuf>,
    pub size: u64,
    /// Exclusão em dispositivo MTP: nó da árvore (o caminho é virtual).
    pub mtp_node: Option<u32>,
}

/// Confirmação da exclusão permanente. `Some(true)` = confirmou, `Some(false)` = cancelou.
pub fn confirm_delete(ctx: &egui::Context, c: &ConfirmDelete) -> Option<bool> {
    let mut result = None;
    egui::Window::new(t!("confirm.title").to_string())
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.colored_label(
                ui.visuals().error_fg_color,
                t!("confirm.permanent").to_string(),
            );
            ui.add_space(6.0);
            for p in &c.paths {
                ui.monospace(p.display().to_string());
            }
            ui.label(t!("confirm.size", size = format_size(c.size)).to_string());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button(
                        egui::RichText::new(t!("confirm.delete"))
                            .color(ui.visuals().error_fg_color),
                    )
                    .clicked()
                {
                    result = Some(true);
                }
                if ui.button(t!("confirm.cancel")).clicked() {
                    result = Some(false);
                }
            });
        });
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        result = Some(false);
    }
    result
}

/// Janela de progresso de uma ação em andamento. Devolve `true` se o usuário pediu cancelamento.
pub fn job_window(ctx: &egui::Context, job: &ActionJob) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let mut cancel = false;
    egui::Window::new(job.label.clone())
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            match &job.progress {
                JobProgress::Native => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t!("job.native").to_string());
                    });
                }
                JobProgress::Zip(p) => {
                    let total = p.bytes_total.load(Relaxed).max(1);
                    let done = p.bytes_done.load(Relaxed);
                    ui.add(
                        egui::ProgressBar::new((done as f64 / total as f64) as f32)
                            .text(format!("{} / {}", format_size(done), format_size(total)))
                            .desired_width(320.0),
                    );
                }
                JobProgress::Compress(p) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t!("job.compress", n = p.done.load(Relaxed)).to_string());
                    });
                }
            }
            if !matches!(job.progress, JobProgress::Native)
                && ui.button(t!("confirm.cancel")).clicked()
            {
                cancel = true;
            }
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    cancel
}
