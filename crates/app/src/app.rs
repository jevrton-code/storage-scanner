use std::path::PathBuf;
use std::time::Duration;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use rust_i18n::t;
use ss_core::Metric;
use ss_core::format::{format_count, format_size};
use ss_ops::file_op::Op;
use ss_ops::{context_menu as native_menu, explorer_integration, open as shell_open, protect};
use ss_scan::mtp::{self, MtpDevice};
use ss_scan::volumes::{self, VolumeInfo};
use ss_scan::{Labels, ScanError, elevation};

use crate::actions::{self, ActionJob, MenuAction};
use crate::state::{JobPoll, ScanJob, ScanResult};
use crate::ui::context_menu::{self, ConfirmDelete, MenuInfo, MenuState};
use crate::ui::dialogs::{self, ErrorDialog};
use crate::ui::ext_panel::ExtPanelState;
use crate::ui::tree_view::TreeViewState;
use crate::ui::treemap_view::TreemapState;
use crate::ui::{status_bar, theme, toolbar};

pub struct App {
    path: String,
    volumes: Vec<VolumeInfo>,
    elevated: bool,
    job: Option<ScanJob>,
    result: Option<ScanResult>,
    tree_view: TreeViewState,
    treemap: TreemapState,
    ext_panel: ExtPanelState,
    highlight_ext: Option<String>,
    metric: Metric,
    error: Option<ErrorDialog>,
    elevate_error: Option<String>,
    /// Alvo recebido na linha de comando, analisado no primeiro quadro.
    pending_start: bool,
    // --- ações de arquivo ---
    hwnd: Option<isize>,
    ctx_menu: Option<MenuState>,
    action: Option<ActionJob>,
    confirm_delete: Option<ConfirmDelete>,
    settings_open: bool,
    explorer_registered: bool,
    notice: Option<String>,
    // --- dispositivos MTP ---
    mtp_devices: Vec<MtpDevice>,
    device_rx: Option<crossbeam_channel::Receiver<Vec<MtpDevice>>>,
    selected_device: Option<MtpDevice>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_target: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        let path = initial_target
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            pending_start: initial_target.is_some(),
            path,
            volumes: volumes::list_volumes(),
            elevated: elevation::is_elevated(),
            job: None,
            result: None,
            tree_view: TreeViewState::new(Metric::Size),
            treemap: TreemapState::new(),
            ext_panel: ExtPanelState::default(),
            highlight_ext: None,
            metric: Metric::Size,
            error: None,
            elevate_error: None,
            hwnd: None,
            ctx_menu: None,
            action: None,
            confirm_delete: None,
            settings_open: false,
            explorer_registered: explorer_integration::is_registered(),
            notice: None,
            mtp_devices: Vec::new(),
            device_rx: None,
            selected_device: None,
        }
        .with_device_scan(&cc.egui_ctx)
    }

    /// Lista os dispositivos MTP em segundo plano (a enumeração pode demorar).
    fn with_device_scan(mut self, ctx: &egui::Context) -> Self {
        self.refresh_devices(ctx);
        self
    }

    fn refresh_devices(&mut self, ctx: &egui::Context) {
        if self.device_rx.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(mtp::list_devices().unwrap_or_default());
            ctx.request_repaint();
        });
        self.device_rx = Some(rx);
    }

    fn labels() -> Labels {
        Labels {
            system_files: t!("tree.system_files").to_string(),
            orphans: t!("tree.orphans").to_string(),
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let target = PathBuf::from(self.path.trim());
        if self.job.is_some() || target.as_os_str().is_empty() {
            return;
        }
        // Um dispositivo MTP só vale enquanto o campo ainda mostra o seu nome.
        let device = self
            .selected_device
            .clone()
            .filter(|d| crate::ui::drive_picker::device_label(d) == self.path);
        self.job = Some(ScanJob::start(ctx, target, device, Self::labels()));
    }

    fn show_error(&mut self, message: String) {
        self.error = Some(ErrorDialog { message });
    }

    fn poll_devices(&mut self) {
        if let Some(rx) = &self.device_rx
            && let Ok(list) = rx.try_recv()
        {
            self.mtp_devices = list;
            self.device_rx = None;
        }
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        match job.poll() {
            JobPoll::Running => {}
            JobPoll::Done(result) => {
                self.tree_view = TreeViewState::new(self.metric);
                self.highlight_ext = None;
                self.treemap = TreemapState::new();
                self.ctx_menu = None;
                self.result = Some(*result);
                self.job = None;
            }
            JobPoll::Failed(e) => {
                self.job = None;
                if !matches!(e, ScanError::Cancelled) {
                    self.show_error(t!("scan.error", error = e.to_string()).to_string());
                }
            }
        }
    }

    /// Acompanha a ação em segundo plano e aplica seu resultado à árvore.
    fn poll_action(&mut self) {
        let Some(job) = &self.action else { return };
        let Some(mut outcome) = job.poll() else {
            return;
        };
        self.action = None;
        if let Some(r) = &mut self.result
            && actions::apply_outcome(r, &mut outcome)
        {
            self.tree_view.invalidate();
            if let Some(sel) = self.tree_view.selected
                && r.tree.is_removed(sel)
            {
                self.tree_view.selected = None;
            }
            if r.tree.is_removed(self.treemap.zoom) {
                self.treemap.zoom = ss_core::Tree::ROOT;
            }
        }
        if outcome.cancelled {
            self.notice = Some(t!("action.cancelled").to_string());
        }
        if let Some(e) = outcome.error {
            self.show_error(t!("action.failed", error = e).to_string());
        } else if let Some(info) = outcome.info {
            self.notice = Some(if info.chars().all(|c| c.is_ascii_digit()) {
                t!("action.compress_partial", n = info).to_string()
            } else {
                t!("action.zip_done", path = info).to_string()
            });
        }
    }

    /// O alvo atual está num volume NTFS local que poderia usar o modo rápido?
    fn wants_elevation(&self) -> bool {
        if self.elevated
            || self.job.as_ref().is_some_and(|j| j.device.is_some())
            || self.result.as_ref().is_some_and(|r| r.mtp.is_some())
        {
            return false;
        }
        let target = self
            .job
            .as_ref()
            .map(|j| j.target.clone())
            .or_else(|| self.result.as_ref().map(|r| r.target.clone()))
            .unwrap_or_else(|| PathBuf::from(self.path.trim()));
        if target.as_os_str().is_empty() {
            return false;
        }
        volumes::volume_root_of(&target)
            .map(|r| volumes::volume_info(&r))
            .is_some_and(|v| v.is_ntfs() && v.kind != volumes::DriveKind::Remote)
    }

    fn banner(&mut self, ui: &mut egui::Ui) {
        if !self.wants_elevation() {
            return;
        }
        egui::Panel::top("elevation_banner").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("⚡ {}", t!("banner.elevation")));
                if ui.button(t!("banner.elevate_button")).clicked() {
                    let args = elevation::quote_arg(self.path.trim());
                    match elevation::relaunch_elevated(&args) {
                        Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.elevate_error = Some(e),
                    }
                }
            });
            if let Some(e) = &self.elevate_error {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    t!("banner.elevate_failed", error = e.as_str()).to_string(),
                );
            }
        });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            let (files, dirs, bytes, _) = job.progress.snapshot();
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.spinner();
                ui.heading(t!("scan.scanning_title", path = job.title()).to_string());
                ui.label(
                    t!(
                        "scan.progress",
                        files = format_count(files),
                        dirs = format_count(dirs),
                        size = format_size(bytes),
                        secs =
                            format!("{:.1}", job.started.elapsed().as_secs_f64()).replace('.', ",")
                    )
                    .to_string(),
                );
            });
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        } else if let Some(r) = &self.result {
            if let Some(reason) = &r.fallback_reason {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    t!("banner.fallback", reason = reason.as_str()).to_string(),
                );
            }
            let events = self.tree_view.show(ui, &r.tree);
            if let Some(node) = events.context_menu {
                self.ctx_menu = Some(MenuState {
                    node,
                    pos: ui.ctx().pointer_interact_pos().unwrap_or_default(),
                });
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.label(t!("app.empty").to_string());
            });
        }
    }

    // ------------------------------------------------------------------ ações

    fn node_path(&self, node: u32) -> Option<PathBuf> {
        let r = self.result.as_ref()?;
        if node as usize >= r.tree.len() || r.tree.is_removed(node) {
            return None;
        }
        Some(PathBuf::from(r.tree.path(node)))
    }

    fn menu_info(&self, node: u32) -> Option<MenuInfo> {
        let r = self.result.as_ref()?;
        let path = self.node_path(node)?;
        let n = r.tree.node(node);
        let mtp = r.mtp.is_some();
        let exists = !mtp && path.exists();
        let ntfs = !mtp
            && volumes::volume_root_of(&path)
                .map(|root| volumes::volume_info(&root).is_ntfs())
                .unwrap_or(false);
        Some(MenuInfo {
            path: path.display().to_string(),
            exists,
            protected: !mtp && protect::is_protected(&path),
            ntfs,
            compressed: n.has(ss_core::flags::COMPRESSED),
            mtp,
        })
    }

    fn run_menu_action(&mut self, ctx: &egui::Context, action: MenuAction, node: u32) {
        let Some(path) = self.node_path(node) else {
            return;
        };
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        if let Some(mtp) = self.result.as_ref().and_then(|r| r.mtp.as_ref()) {
            let object_id = mtp
                .object_ids
                .get(node as usize)
                .cloned()
                .unwrap_or_default();
            let device = mtp.device.clone();
            match action {
                MenuAction::CopyPath => ctx.copy_text(path.display().to_string()),
                MenuAction::DeletePermanent => {
                    let size = self.result.as_ref().map_or(0, |r| r.tree.node(node).size);
                    self.confirm_delete = Some(ConfirmDelete {
                        paths: vec![path],
                        size,
                        mtp_node: Some(node),
                    });
                }
                MenuAction::CopyTo => {
                    if let Some(dest) = rfd::FileDialog::new().pick_folder() {
                        self.start_action(ActionJob::mtp_copy_to_pc(
                            ctx,
                            t!("job.copy", name = name).to_string(),
                            device,
                            object_id,
                            dest,
                        ));
                    }
                }
                _ => {}
            }
            return;
        }
        match action {
            MenuAction::Open => {
                if let Err(e) = shell_open::open(&path, self.elevated) {
                    self.show_error(t!("action.failed", error = e.to_string()).to_string());
                }
            }
            MenuAction::Reveal => {
                if let Err(e) = shell_open::reveal(&path) {
                    self.show_error(t!("action.failed", error = e.to_string()).to_string());
                }
            }
            MenuAction::CopyPath => ctx.copy_text(path.display().to_string()),
            MenuAction::Recycle => {
                self.start_action(ActionJob::file_op(
                    ctx,
                    t!("job.recycle", name = name).to_string(),
                    Op::Delete { permanent: false },
                    vec![path],
                    self.hwnd,
                ));
            }
            MenuAction::DeletePermanent => {
                let size = self.result.as_ref().map_or(0, |r| r.tree.node(node).size);
                self.confirm_delete = Some(ConfirmDelete {
                    paths: vec![path],
                    size,
                    mtp_node: None,
                });
            }
            MenuAction::MoveTo | MenuAction::CopyTo => {
                let Some(dest) = rfd::FileDialog::new().pick_folder() else {
                    return;
                };
                let (op, label) = if action == MenuAction::MoveTo {
                    (Op::Move { dest_dir: dest }, t!("job.move", name = name))
                } else {
                    (Op::Copy { dest_dir: dest }, t!("job.copy", name = name))
                };
                self.start_action(ActionJob::file_op(
                    ctx,
                    label.to_string(),
                    op,
                    vec![path],
                    self.hwnd,
                ));
            }
            MenuAction::Zip => {
                self.start_action(ActionJob::zip(
                    ctx,
                    t!("job.zip", name = name).to_string(),
                    vec![path],
                ));
            }
            MenuAction::Compress(enable) => {
                let label = if enable {
                    t!("job.ntfs_compress", name = name)
                } else {
                    t!("job.ntfs_uncompress", name = name)
                };
                self.start_action(ActionJob::ntfs_compress(
                    ctx,
                    label.to_string(),
                    path,
                    enable,
                ));
            }
            MenuAction::NativeMenu => {
                if let Some(hwnd) = self.hwnd {
                    let (x, y) = native_menu::cursor_screen_pos();
                    if let Err(e) = native_menu::show(hwnd, &path, x, y) {
                        self.show_error(t!("action.failed", error = e.to_string()).to_string());
                    }
                    // O menu do Windows pode ter alterado o item: relê-o.
                    self.refresh_path(ctx, path);
                }
            }
        }
    }

    fn start_action(&mut self, job: ActionJob) {
        if self.action.is_none() {
            self.action = Some(job);
        }
    }

    /// Relê um item (e o enxerta de volta) depois de uma ação feita fora do app.
    fn refresh_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        let name = path.display().to_string();
        let job = ActionJob::refresh(ctx, name, path);
        self.start_action(job);
    }

    fn handle_context_menu(&mut self, ctx: &egui::Context) {
        let Some(state) = &self.ctx_menu else { return };
        let node = state.node;
        let Some(info) = self.menu_info(node) else {
            self.ctx_menu = None;
            return;
        };
        let busy = self.action.is_some();
        let (action, close) = context_menu::show(ctx, state, &info);
        if close {
            self.ctx_menu = None;
        }
        if let Some(a) = action
            && !busy
        {
            self.run_menu_action(ctx, a, node);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.hwnd.is_none()
            && let Ok(h) = frame.window_handle()
            && let RawWindowHandle::Win32(w) = h.as_raw()
        {
            self.hwnd = Some(w.hwnd.get());
        }
        self.poll_devices();
        self.poll_job();
        self.poll_action();
        if self.pending_start {
            self.pending_start = false;
            self.start_scan(&ctx);
        }

        let mut tb = toolbar::ToolbarOutput::default();
        egui::Panel::top("toolbar").show(ui, |ui| {
            tb = toolbar::show(
                ui,
                &mut self.path,
                &self.volumes,
                &self.mtp_devices,
                self.job.is_some(),
                &mut self.metric,
            );
        });
        if self.metric != self.tree_view.metric {
            self.tree_view.set_metric(self.metric);
        }
        if let Some(d) = tb.picked_device.take() {
            self.selected_device = Some(d);
        }
        if tb.refresh_drives {
            self.volumes = volumes::list_volumes();
            self.refresh_devices(&ctx);
        }
        if tb.open_settings {
            self.settings_open = true;
        }
        if tb.stop
            && let Some(job) = &self.job
        {
            job.stop();
        }
        if tb.analyze {
            self.start_scan(&ctx);
        }

        self.banner(ui);

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            let notice = self.notice.clone();
            status_bar::show(
                ui,
                self.result.as_ref(),
                self.tree_view.selected,
                notice.as_deref(),
            );
        });

        if self.job.is_none()
            && let Some(r) = &self.result
        {
            egui::Panel::bottom("treemap")
                .resizable(true)
                .default_size(320.0)
                .min_size(120.0)
                .show(ui, |ui| {
                    let ev = self.treemap.show(
                        ui,
                        &r.tree,
                        r.generation,
                        self.metric,
                        self.tree_view.selected,
                        self.highlight_ext.as_deref(),
                    );
                    if let Some(id) = ev.select {
                        self.tree_view.reveal(&r.tree, id);
                    }
                    if let Some(node) = ev.context {
                        self.ctx_menu = Some(MenuState {
                            node,
                            pos: ui.ctx().pointer_interact_pos().unwrap_or_default(),
                        });
                    }
                });
        }

        if self.job.is_none()
            && let Some(r) = &self.result
        {
            egui::Panel::right("ext_panel")
                .resizable(true)
                .default_size(360.0)
                .min_size(220.0)
                .show(ui, |ui| {
                    self.ext_panel.show(
                        ui,
                        &r.tree,
                        &r.ext_stats,
                        r.generation,
                        self.metric,
                        &mut self.highlight_ext,
                    );
                });
        }

        egui::CentralPanel::default().show(ui, |ui| self.central(ui));

        self.handle_context_menu(&ctx);

        if let Some(c) = &self.confirm_delete {
            match context_menu::confirm_delete(&ctx, c) {
                Some(true) => {
                    let paths = c.paths.clone();
                    let mtp_node = c.mtp_node;
                    self.confirm_delete = None;
                    let label = t!("job.delete_permanent").to_string();
                    let mtp = self.result.as_ref().and_then(|r| r.mtp.as_ref());
                    if let (Some(node), Some(m)) = (mtp_node, mtp) {
                        let object_id =
                            m.object_ids.get(node as usize).cloned().unwrap_or_default();
                        let device = m.device.clone();
                        let tree_path = paths.first().cloned().unwrap_or_default();
                        self.start_action(ActionJob::mtp_delete(
                            &ctx, label, device, object_id, tree_path,
                        ));
                    } else {
                        self.start_action(ActionJob::file_op(
                            &ctx,
                            label,
                            Op::Delete { permanent: true },
                            paths,
                            self.hwnd,
                        ));
                    }
                }
                Some(false) => self.confirm_delete = None,
                None => {}
            }
        }

        if let Some(job) = &self.action
            && context_menu::job_window(&ctx, job)
        {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        if self.settings_open {
            let mut registered = self.explorer_registered;
            if dialogs::show_settings(&ctx, &mut self.settings_open, &mut registered) {
                let res = if registered {
                    std::env::current_exe()
                        .map_err(ss_ops::OpError::from)
                        .and_then(|exe| {
                            explorer_integration::register(&exe, &t!("explorer.menu_label"))
                        })
                } else {
                    explorer_integration::unregister()
                };
                match res {
                    Ok(()) => self.explorer_registered = registered,
                    Err(e) => {
                        self.show_error(t!("action.failed", error = e.to_string()).to_string())
                    }
                }
            }
        }

        if let Some(err) = &self.error
            && dialogs::show_error(&ctx, err)
        {
            self.error = None;
        }
    }
}
