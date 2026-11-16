use std::path::PathBuf;
use std::time::Duration;

use rust_i18n::t;
use ss_core::Metric;
use ss_core::format::{format_count, format_size};
use ss_scan::volumes::{self, VolumeInfo};
use ss_scan::{Labels, ScanError, elevation};

use crate::state::{JobPoll, ScanJob, ScanResult};
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
        }
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
        self.job = Some(ScanJob::start(ctx, target, Self::labels()));
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        match job.poll() {
            JobPoll::Running => {}
            JobPoll::Done(result) => {
                self.tree_view = TreeViewState::new(self.metric);
                self.highlight_ext = None;
                self.treemap = TreemapState::new();
                self.result = Some(*result);
                self.job = None;
            }
            JobPoll::Failed(e) => {
                self.job = None;
                if !matches!(e, ScanError::Cancelled) {
                    self.error = Some(ErrorDialog {
                        message: t!("scan.error", error = e.to_string()).to_string(),
                    });
                }
            }
        }
    }

    /// O alvo atual está num volume NTFS local que poderia usar o modo rápido?
    fn wants_elevation(&self) -> bool {
        if self.elevated {
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
                ui.heading(
                    t!(
                        "scan.scanning_title",
                        path = job.target.display().to_string()
                    )
                    .to_string(),
                );
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
            let _ = events; // seleção/menu de contexto serão usados por treemap e ações
        } else {
            ui.centered_and_justified(|ui| {
                ui.label(t!("app.empty").to_string());
            });
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job();
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
                self.job.is_some(),
                &mut self.metric,
            );
        });
        if self.metric != self.tree_view.metric {
            self.tree_view.set_metric(self.metric);
        }
        if tb.refresh_drives {
            self.volumes = volumes::list_volumes();
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
            status_bar::show(ui, self.result.as_ref(), self.tree_view.selected);
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

        if let Some(err) = &self.error
            && dialogs::show_error(&ctx, err)
        {
            self.error = None;
        }
    }
}
