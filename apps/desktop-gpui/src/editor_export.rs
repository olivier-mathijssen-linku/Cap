//! Editor export page -- `routes/editor/ExportPage.tsx`, 1:1 layout.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cap_export::estimates::{ExportEstimates, estimate_export};
use cap_export::gif::GifExportSettings;
use cap_export::mov::MovExportSettings;
use cap_export::mp4::{ExportCompression, Mp4ExportSettings};
use cap_export::preview::{
    ExportPreviewSettings, render_preview_with_config, render_preview_with_editor,
};
use cap_export::settings::ExportSettings;
use cap_export::{ExporterBase, make_cursor_only_project};
use cap_project::{BackgroundSource, RecordingMeta, XY};
use cap_utils::export_resources::{
    DiskBudget, ExportResources, estimated_working_bytes, is_resource_stop,
};
use gpui::{
    AppContext as _, Bounds, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, RenderImage, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, svg,
};

use crate::editor_window::EditorWindow;
use crate::sign_in::{SignIn, SignInEvent, SignInOrigin, SignInOutcome};
use crate::store::{self, ExportPrefs};
use crate::ui;
use crate::{library, platform};

pub(crate) const EXPORT_PANEL_WIDTH: f32 = 416.;
pub(crate) const EXPORT_PANEL_GAP: f32 = 8.;
const SEGMENT_HEIGHT: f32 = 26.;
const COMPRESSION_PRESETS: [(ExportCompression, &str); 4] = [
    (ExportCompression::Potato, "Klein"),
    (ExportCompression::Web, "Web"),
    (ExportCompression::Social, "Social"),
    (ExportCompression::Maximum, "Max"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportDestination {
    File,
    Clipboard,
    Link,
}

impl ExportDestination {
    const ALL: &'static [Self] = &[Self::File, Self::Clipboard, Self::Link];

    fn label(self) -> &'static str {
        match self {
            Self::File => "Bestand",
            Self::Clipboard => "Klembord",
            Self::Link => "Deelbare link",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::File => "icons/folder.svg",
            Self::Clipboard => "icons/copy.svg",
            Self::Link => "icons/link.svg",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Clipboard => "clipboard",
            Self::Link => "link",
        }
    }

    fn from_slug(slug: &str) -> Self {
        match slug {
            "clipboard" => Self::Clipboard,
            "link" => Self::Link,
            _ => Self::File,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormatKind {
    Mp4,
    Gif,
}

impl ExportFormatKind {
    fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Gif => "GIF",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::Mp4 => "Mp4",
            Self::Gif => "Gif",
        }
    }

    fn from_slug(slug: &str) -> Self {
        match slug {
            "Gif" => Self::Gif,
            _ => Self::Mp4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportResolution {
    P720,
    P1080,
    P4k,
}

impl ExportResolution {
    fn label(self) -> &'static str {
        match self {
            Self::P720 => "720p",
            Self::P1080 => "1080p",
            Self::P4k => "4K",
        }
    }

    fn size(self) -> (u32, u32) {
        match self {
            Self::P720 => (1280, 720),
            Self::P1080 => (1920, 1080),
            Self::P4k => (3840, 2160),
        }
    }

    fn from_slug(slug: &str) -> Self {
        match slug {
            "1080p" => Self::P1080,
            "4K" | "4k" => Self::P4k,
            _ => Self::P720,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportPhase {
    Idle,
    ChoosingFile,
    Starting,
    Rendering,
    Copying,
    Uploading,
    Done,
    Failed,
}

impl ExportPhase {
    pub(crate) fn is_busy(self) -> bool {
        matches!(
            self,
            Self::ChoosingFile | Self::Starting | Self::Rendering | Self::Copying | Self::Uploading
        )
    }

    fn shows_progress(self) -> bool {
        !matches!(self, Self::Idle | Self::ChoosingFile)
    }
}

/// The four settings the export panel shows as chips, each with its own
/// quick menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickSetting {
    Format,
    Resolution,
    Fps,
    Quality,
}

impl QuickSetting {
    const ALL: [Self; 4] = [Self::Format, Self::Resolution, Self::Fps, Self::Quality];

    fn index(self) -> usize {
        match self {
            Self::Format => 0,
            Self::Resolution => 1,
            Self::Fps => 2,
            Self::Quality => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Format => "Formaat",
            Self::Resolution => "Resolutie",
            Self::Fps => "Framerate",
            Self::Quality => "Kwaliteit",
        }
    }
}

pub struct QuickMenu {
    setting: QuickSetting,
    state: ui::MenuState,
}

/// The Drive folder an upload goes to, with the names leading to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveChoice {
    pub id: String,
    pub path: String,
}

enum FolderEntries {
    Loading,
    Loaded(Vec<cap_direct_drive::DriveFolder>),
    Failed(String),
    NeedsReconnect,
}

/// The "Opslaan in Drive" browser: `trail` is the folders opened from the
/// top level (My Drive plus the shared drives), `selected` a folder in the
/// current list.
pub struct FolderPicker {
    trail: Vec<cap_direct_drive::DriveFolder>,
    entries: FolderEntries,
    selected: Option<cap_direct_drive::DriveFolder>,
    creating: bool,
    searching: bool,
    search_input: gpui::Entity<ui::TextInputState>,
    name_input: gpui::Entity<ui::TextInputState>,
    task: Option<gpui::Task<()>>,
    generation: u64,
    _subscriptions: [gpui::Subscription; 2],
}

pub struct PreviewStats {
    pub estimated_size_mb: f64,
}

pub struct ExportUi {
    pub destination: ExportDestination,
    pub format: ExportFormatKind,
    pub resolution: ExportResolution,
    pub fps: u32,
    pub compression: ExportCompression,
    pub optimize_filesize: bool,
    pub cursor_only: bool,
    pub custom_bpp: Option<f32>,
    pub force_ffmpeg: bool,
    pub advanced_open: bool,
    pub preview: Option<Arc<RenderImage>>,
    pub preview_stats: Option<PreviewStats>,
    pub preview_error: Option<String>,
    pub preview_task: Option<gpui::Task<()>>,
    preview_request: Arc<()>,
    estimate: Option<ExportEstimates>,
    estimate_loading: bool,
    estimate_cancel: Arc<AtomicBool>,
    pub phase: ExportPhase,
    close_requested: bool,
    pub rendered: u32,
    pub total_frames: u32,
    pub output_path: Option<PathBuf>,
    pub error: Option<String>,
    pub cancel: Arc<AtomicBool>,
    pub export_task: Option<gpui::Task<()>>,
    /// The server answered the last share with 401: the stored credential is
    /// dead even though one exists, so the CTA offers sign-in again instead of
    /// another upload that would fail the same way.
    auth_rejected: bool,
    sign_in_subscriptions: Option<[gpui::Subscription; 2]>,
    pub organization_id: Option<String>,
    pub share_link: Option<String>,
    pub reuploading: bool,
    pub upload_progress: f32,
    pub copy_link_pressed: bool,
    bpp_track: ui::SliderTrack,
    bpp_dragging: bool,
    quick_menu: Option<QuickMenu>,
    settings_open: bool,
    chip_bounds: [Rc<Cell<Option<Bounds<Pixels>>>>; 4],
    pub drive_folder: Option<DriveChoice>,
    team_folder: Option<DriveChoice>,
    team_loading: bool,
    team_error: Option<String>,
    team_task: Option<gpui::Task<()>>,
    folder_picker: Option<FolderPicker>,
}

impl Drop for ExportUi {
    fn drop(&mut self) {
        self.estimate_cancel.store(true, Ordering::Release);
    }
}

const BPP_MIN: f32 = 0.02;
const BPP_MAX: f32 = 0.5;
const BPP_STEP: f32 = 0.01;

impl ExportUi {
    fn pick_default_organization(&mut self) {
        let organizations = store::auth_snapshot().organizations;
        if self
            .organization_id
            .as_ref()
            .is_some_and(|id| organizations.iter().any(|org| &org.id == id))
        {
            return;
        }
        if let Some(org) = organizations.first() {
            self.organization_id = Some(org.id.clone());
            self.persist();
        }
    }

    pub fn load() -> Self {
        let prefs = store::load().export.unwrap_or(ExportPrefs {
            format: "Mp4".into(),
            fps: 30,
            export_to: "file".into(),
            resolution: "720p".into(),
            compression: "Maximum".into(),
            optimize_filesize: false,
            cursor_only: false,
            custom_bpp: None,
            force_ffmpeg_decoder: false,
            advanced_open: false,
            organization_id: None,
            drive_folder_id: None,
            drive_folder_path: None,
        });
        Self::from_preferences(prefs)
    }

    fn from_preferences(prefs: ExportPrefs) -> Self {
        Self {
            destination: ExportDestination::from_slug(&prefs.export_to),
            format: ExportFormatKind::from_slug(&prefs.format),
            resolution: ExportResolution::from_slug(&prefs.resolution),
            fps: prefs.fps,
            compression: match prefs.compression.as_str() {
                "Social" => ExportCompression::Social,
                "Web" => ExportCompression::Web,
                "Potato" => ExportCompression::Potato,
                _ => ExportCompression::Maximum,
            },
            optimize_filesize: prefs.optimize_filesize,
            cursor_only: prefs.cursor_only,
            custom_bpp: prefs.custom_bpp,
            force_ffmpeg: prefs.force_ffmpeg_decoder,
            advanced_open: prefs.advanced_open,
            preview: None,
            preview_stats: None,
            preview_error: None,
            preview_task: None,
            preview_request: Arc::new(()),
            estimate: None,
            estimate_loading: false,
            estimate_cancel: Arc::new(AtomicBool::new(false)),
            phase: ExportPhase::Idle,
            close_requested: false,
            rendered: 0,
            total_frames: 0,
            output_path: None,
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
            export_task: None,
            auth_rejected: false,
            sign_in_subscriptions: None,
            organization_id: prefs.organization_id,
            share_link: None,
            reuploading: false,
            upload_progress: 0.0,
            copy_link_pressed: false,
            bpp_track: ui::SliderTrack::default(),
            bpp_dragging: false,
            quick_menu: None,
            settings_open: false,
            chip_bounds: std::array::from_fn(|_| Rc::new(Cell::new(None))),
            drive_folder: prefs.drive_folder_id.clone().map(|id| DriveChoice {
                id,
                path: prefs.drive_folder_path.clone().unwrap_or_default(),
            }),
            team_folder: None,
            team_loading: false,
            team_error: None,
            team_task: None,
            folder_picker: None,
        }
    }

    fn update_preview(&mut self, request: &Arc<()>, update: impl FnOnce(&mut Self)) -> bool {
        if !Arc::ptr_eq(&self.preview_request, request) {
            return false;
        }
        update(self);
        true
    }

    fn persist(&self) {
        let prefs = ExportPrefs {
            format: self.format.slug().to_string(),
            fps: self.fps,
            export_to: self.destination.slug().to_string(),
            resolution: self.resolution.label().to_string(),
            compression: format!("{:?}", self.compression),
            optimize_filesize: self.optimize_filesize,
            cursor_only: self.cursor_only,
            custom_bpp: self.custom_bpp,
            force_ffmpeg_decoder: self.force_ffmpeg,
            advanced_open: self.advanced_open,
            organization_id: self.organization_id.clone(),
            drive_folder_id: self.drive_folder.as_ref().map(|choice| choice.id.clone()),
            drive_folder_path: self.drive_folder.as_ref().map(|choice| choice.path.clone()),
        };
        store::update(|state| state.export = Some(prefs));
    }

    fn bpp(&self) -> f32 {
        self.custom_bpp
            .unwrap_or_else(|| self.compression.bits_per_pixel())
    }

    fn estimate_settings(&self) -> ExportSettings {
        let (width, height) = self.resolution.size();
        let resolution_base = XY::new(width, height);
        if self.cursor_only {
            ExportSettings::Mov(MovExportSettings {
                fps: self.fps,
                resolution_base,
                cursor_only: true,
            })
        } else if self.format == ExportFormatKind::Gif {
            ExportSettings::Gif(GifExportSettings {
                fps: self.fps,
                resolution_base,
                quality: None,
            })
        } else {
            ExportSettings::Mp4(Mp4ExportSettings {
                fps: self.fps,
                resolution_base,
                compression: self.compression,
                custom_bpp: self.custom_bpp,
                force_ffmpeg_decoder: self.force_ffmpeg,
                optimize_filesize: self.optimize_filesize,
            })
        }
    }

    fn is_custom_bpp(&self) -> bool {
        self.custom_bpp
            .is_some_and(|bpp| (bpp - self.compression.bits_per_pixel()).abs() > 0.001)
    }

    fn complete_share_export(&mut self, link: String, copy_link: impl FnOnce(String)) {
        if self.close_requested || self.cancel.load(Ordering::Relaxed) {
            self.phase = ExportPhase::Idle;
            return;
        }

        self.phase = ExportPhase::Done;
        self.share_link = Some(link.clone());
        self.upload_progress = 1.0;
        copy_link(link);
    }

    fn clipboard_retry_path(&self) -> Option<&Path> {
        if self.phase == ExportPhase::Failed && self.destination == ExportDestination::Clipboard {
            self.output_path.as_deref()
        } else {
            None
        }
    }

    fn copy_completed_export(
        &mut self,
        path: PathBuf,
        copy: impl FnOnce(&Path) -> Result<(), String>,
    ) {
        self.output_path = Some(path.clone());
        self.error = None;
        if self.close_requested || self.cancel.load(Ordering::Relaxed) {
            self.phase = ExportPhase::Idle;
            return;
        }

        self.phase = ExportPhase::Copying;
        match copy(&path) {
            Ok(()) => self.phase = ExportPhase::Done,
            Err(error) => {
                self.phase = ExportPhase::Failed;
                self.error = Some(error);
            }
        }
    }
}

fn has_transparent_background(project: &cap_project::ProjectConfiguration) -> bool {
    matches!(
        project.background.source,
        BackgroundSource::Color { alpha, .. } if alpha < 255
    )
}

fn export_file_name(pretty_name: &str, extension: &str) -> String {
    let mut name = sanitize_filename::sanitize_with_options(
        pretty_name,
        sanitize_filename::Options {
            windows: true,
            ..Default::default()
        },
    );
    while name.len() > 255 - extension.len() - 1 {
        let _ = name.pop();
    }
    if name.trim().is_empty() {
        name = "Kiekes Recording".into();
    }
    format!("{name}.{extension}")
}

fn decode_jpeg_bytes(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let rgba = cap_gpui_kernels::codec::decode_jpeg_to_bgra(bytes)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(rgba)
    ])))
}

impl EditorWindow {
    pub(crate) fn render_reupload_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .rounded(px(8.))
            .bg(Hsla::from(self.theme.blue_3))
            .border_1()
            .border_color(Hsla::from(self.theme.blue_5))
            .child(
                ui::EditorButton::plain(&self.theme, "editor-reupload")
                    .left_icon("icons/cloud-upload.svg")
                    .label("Opnieuw uploaden")
                    .tooltip(
                        &self.theme,
                        if has_transparent_background(&self.project) {
                            "Share links require a background without transparency"
                        } else {
                            "Upload your latest edit to the same link"
                        },
                    )
                    .disabled(!self.project_ready() || has_transparent_background(&self.project))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if !this.project_ready() {
                            return;
                        }
                        this.ensure_export(window, cx);
                        if !has_transparent_background(&this.project)
                            && let Some(ui) = this.export.as_mut()
                        {
                            ui.destination = ExportDestination::Link;
                            ui.format = ExportFormatKind::Mp4;
                            ui.cursor_only = false;
                        }
                        this.normalize_loaded_export_fps();
                        this.refresh_export_preview(window, cx);
                        cx.notify();
                    })),
            )
    }

    pub(crate) fn ensure_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.export.is_some() {
            return;
        }
        let mut ui = ExportUi::load();
        if let Ok(meta) = RecordingMeta::load_for_project(&self.project_path) {
            self.sharing = meta.sharing;
        }
        if has_transparent_background(&self.project) {
            ui.format = ExportFormatKind::Gif;
            if ui.resolution == ExportResolution::P4k {
                ui.resolution = ExportResolution::P1080;
            }
            if ui.destination == ExportDestination::Link {
                ui.destination = ExportDestination::File;
            }
        }
        if ui.cursor_only && ui.destination == ExportDestination::Link {
            ui.destination = ExportDestination::File;
        }
        self.normalize_export_fps(&mut ui);
        self.export = Some(ui);
        self.subscribe_export_to_sign_in(window, cx);
        self.refresh_export_preview(window, cx);
        self.load_team_destination(cx);
        cx.notify();
    }

    pub(crate) fn reset_export(&mut self, cx: &mut Context<Self>) {
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        if ui.phase.is_busy() {
            return;
        }
        ui.phase = ExportPhase::Idle;
        ui.rendered = 0;
        ui.total_frames = 0;
        ui.error = None;
        ui.upload_progress = 0.;
        ui.copy_link_pressed = false;
        cx.notify();
    }

    pub(crate) fn close_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ui) = self.export.as_mut() {
            if ui.phase.is_busy() {
                ui.close_requested = true;
                ui.cancel.store(true, Ordering::Relaxed);
                cx.notify();
                return;
            }
            if let Some(image) = ui.preview.take() {
                let _ = window.drop_image(image);
            }
            ui.persist();
        }
        self.export = None;
        cx.notify();
    }

    fn finish_requested_export_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .export
            .as_ref()
            .is_some_and(|ui| ui.close_requested && !ui.phase.is_busy())
        {
            self.close_export(window, cx);
        }
    }

    fn copy_share_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(link) = self.export.as_ref().and_then(|ui| ui.share_link.clone()) else {
            return;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(link));
        if let Some(ui) = self.export.as_mut() {
            ui.copy_link_pressed = true;
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(ui) = this.export.as_mut() {
                    ui.copy_link_pressed = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn normalize_export_fps(&self, ui: &mut ExportUi) {
        let allowed = self.export_fps_options(ui);
        if !allowed.contains(&ui.fps) {
            ui.fps = if ui.format == ExportFormatKind::Gif {
                15
            } else {
                30
            };
        }
    }

    fn normalize_loaded_export_fps(&mut self) {
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        let gif = ui.format == ExportFormatKind::Gif;
        let allowed = if gif && !ui.cursor_only {
            &[10, 15, 20, 25, 30][..]
        } else {
            &[15, 30, 60][..]
        };
        if !allowed.contains(&ui.fps) {
            ui.fps = if gif { 15 } else { 30 };
        }
    }

    fn export_fps_options(&self, ui: &ExportUi) -> &'static [u32] {
        if ui.format == ExportFormatKind::Gif && !ui.cursor_only {
            &[10, 15, 20, 25, 30]
        } else {
            &[15, 30, 60]
        }
    }

    fn export_resolutions(&self, ui: &ExportUi) -> Vec<ExportResolution> {
        let transparent = has_transparent_background(&self.project);
        if ui.format == ExportFormatKind::Gif || transparent || ui.cursor_only {
            vec![ExportResolution::P720, ExportResolution::P1080]
        } else {
            vec![
                ExportResolution::P720,
                ExportResolution::P1080,
                ExportResolution::P4k,
            ]
        }
    }

    fn refresh_export_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        let time = self.preview_or_playhead();
        let instance = self.instance.clone();
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        let project = self.project.clone();
        ui.estimate_cancel.store(true, Ordering::Release);
        ui.estimate_cancel = Arc::new(AtomicBool::new(false));
        ui.estimate = None;
        ui.estimate_loading = true;
        ui.preview_stats = None;
        let estimate_cancel = ui.estimate_cancel.clone();
        let estimate_settings = ui.estimate_settings();
        let estimate_instance = instance.clone();
        let estimate_project = project.clone();
        let (width, height) = ui.resolution.size();
        let settings = ExportPreviewSettings {
            fps: ui.fps,
            resolution_base: XY::new(width, height),
            compression_bpp: ui.bpp(),
            cursor_only: ui.cursor_only,
        };
        // Match Windows editor playback: a fresh Media Foundation preview seek can return black.
        let force = cfg!(target_os = "windows") || ui.force_ffmpeg;
        ui.preview_error = None;
        let request = Arc::new(());
        ui.preview_request = request.clone();
        ui.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            let started = std::time::Instant::now();
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                match instance {
                    Some(instance) => {
                        render_preview_with_editor(&instance, project, time, settings).await
                    }
                    None => render_preview_with_config(path, project, time, settings, force).await,
                }
            })
            .await
            .ok();
            tracing::debug!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                "export preview rendered"
            );
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(ui) = this.export.as_mut() else {
                    return;
                };
                let updated = ui.update_preview(&request, |ui| match result {
                    Some(Ok(preview)) => {
                        let bytes = base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            preview.jpeg_base64.as_bytes(),
                        )
                        .ok();
                        if let Some(image) = bytes.as_deref().and_then(decode_jpeg_bytes)
                            && let Some(old) = ui.preview.replace(image)
                        {
                            let _ = window.drop_image(old);
                        }
                        ui.preview_stats = Some(PreviewStats {
                            estimated_size_mb: preview.estimated_size_mb,
                        });
                        ui.preview_error = None;
                    }
                    Some(Err(error)) => {
                        ui.preview_error = Some(error.to_string());
                    }
                    None => {
                        ui.preview_error = Some("Preview unavailable".into());
                    }
                });
                if !updated {
                    return;
                }
                cx.notify();
                window.refresh();
            });
            let result = if let Some(instance) = estimate_instance {
                let (estimate_tx, estimate_rx) = flume::unbounded();
                let estimate_task = gpui_tokio::Tokio::spawn(cx, async move {
                    estimate_export(
                        instance,
                        estimate_project,
                        estimate_settings,
                        estimate_cancel,
                        move |estimate| {
                            let _ = estimate_tx.send(estimate);
                        },
                    )
                    .await
                });
                while let Ok(estimate) = estimate_rx.recv_async().await {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(ui) = this.export.as_mut()
                            && !ui.estimate_cancel.load(Ordering::Acquire)
                            && ui.update_preview(&request, |ui| ui.estimate = Some(estimate))
                        {
                            cx.notify();
                        }
                    });
                }
                estimate_task.await.ok().and_then(Result::ok)
            } else {
                None
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(ui) = this.export.as_mut()
                    && ui.update_preview(&request, |ui| {
                        if let Some(estimate) = result {
                            ui.estimate = Some(estimate);
                        }
                        ui.estimate_loading = false;
                    })
                {
                    cx.notify();
                }
            });
        }));
    }

    pub(crate) fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.playing && !self.export.as_ref().is_some_and(|ui| ui.phase.is_busy()) {
            self.toggle_play_from_crop(cx);
        }
        let pretty_name = self
            .summary()
            .map(|summary| summary.pretty_name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Kiekes Recording".into());
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        ui.estimate_cancel.store(true, Ordering::Release);
        if ui.phase.is_busy() || ui.close_requested {
            return;
        }
        ui.settings_open = false;
        ui.quick_menu = None;
        ui.folder_picker = None;
        if ui.destination == ExportDestination::Link {
            if crate::direct_drive::enabled() {
                if !cap_direct_drive::is_connected() {
                    cx.defer(|cx| {
                        crate::app_windows::open_settings(
                            crate::settings_window::Page::Integrations,
                            cx,
                        )
                    });
                    return;
                }
                self.start_link_export(window, cx);
                return;
            }
            let sign_in = SignIn::global(cx);
            if sign_in.read(cx).is_pending() {
                sign_in.update(cx, |sign_in, cx| sign_in.cancel(cx));
                return;
            }
            if ui.auth_rejected || !store::auth_snapshot().signed_in() {
                sign_in.update(cx, |sign_in, cx| sign_in.start(SignInOrigin::Editor, cx));
                return;
            }
            self.start_link_export(window, cx);
            return;
        }

        let estimate = ui
            .estimate
            .as_ref()
            .map(|estimate| estimate.size_range_mb[1])
            .or_else(|| {
                ui.preview_stats
                    .as_ref()
                    .map(|stats| stats.estimated_size_mb)
            })
            .map(estimated_working_bytes)
            .unwrap_or(0);
        let destination = ui.destination;
        let format = ui.format;
        let cursor_only = ui.cursor_only;
        let fps = ui.fps;
        let (width, height) = ui.resolution.size();
        let compression = ui.compression;
        let custom_bpp = ui.custom_bpp;
        let optimize = ui.optimize_filesize;
        let force = ui.force_ffmpeg;
        let project_path = self.project_path.clone();
        let project = self.project.clone();
        ui.cancel = Arc::new(AtomicBool::new(false));
        let cancel = ui.cancel.clone();
        ui.phase = if destination == ExportDestination::File {
            ExportPhase::ChoosingFile
        } else {
            ExportPhase::Starting
        };
        ui.error = None;
        ui.rendered = 0;
        ui.total_frames = 0;
        ui.output_path = None;
        cx.notify();

        ui.export_task = Some(cx.spawn_in(window, async move |this, cx| {
            let save_path = if destination == ExportDestination::File {
                let ext = if cursor_only {
                    "mov"
                } else if format == ExportFormatKind::Gif {
                    "gif"
                } else {
                    "mp4"
                };
                let default = export_file_name(&pretty_name, ext);
                let chosen: Result<Option<PathBuf>, String> = match std::env::var_os("CAP_GPUI_AUTO_EXPORT") {
                    Some(path) => Ok(Some(PathBuf::from(path))),
                    None => {
                        #[cfg(target_os = "macos")]
                        {
                            platform::try_save_file_panel(&default, &[ext])
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            Ok(platform::save_file_panel_async(&default, &[ext], cx).await)
                        }
                    }
                };
                if matches!(chosen, Ok(None)) {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(ui) = this.export.as_mut()
                            && Arc::ptr_eq(&ui.cancel, &cancel)
                        {
                            ui.phase = ExportPhase::Idle;
                        }
                        cx.notify();
                    });
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.finish_requested_export_close(window, cx)
                    });
                    return;
                }
                match chosen {
                    Ok(path) => path,
                    Err(error) => {
                        tracing::warn!(error, "Save dialog unavailable; keeping the export in its project output folder");
                        None
                    }
                }
            } else {
                None
            };

            let output = save_path.clone().unwrap_or_else(|| project_path.join("output"));
            let resources = ExportResources::new(vec![DiskBudget {
                path: output,
                description: "your export drive",
                estimated_bytes: estimate,
            }]);
            if !confirm_export_resources(&this, cx, &resources, &cancel).await {
                let _ = this.update_in(cx, |this, window, cx| this.finish_requested_export_close(window, cx));
                return;
            }

            let started = this.update_in(cx, |this, _, cx| {
                let Some(ui) = this.export.as_mut() else {
                    return false;
                };
                if !Arc::ptr_eq(&ui.cancel, &cancel) {
                    return false;
                }
                if cancel.load(Ordering::Relaxed) {
                    ui.phase = ExportPhase::Idle;
                    cx.notify();
                    return false;
                }
                ui.phase = ExportPhase::Starting;
                cx.notify();
                true
            });
            if !started.unwrap_or(false) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_requested_export_close(window, cx)
                });
                return;
            }

            let notify_file_save = save_path.is_some();
            let (progress_tx, progress_rx) = flume::unbounded::<(u32, u32)>();
            let export_cancel = cancel.clone();
            let export = gpui_tokio::Tokio::spawn(cx, async move {
                let stopped = export_cancel.clone();
                resources.supervise(run_export(
                    project_path,
                    project,
                    format,
                    cursor_only,
                    fps,
                    width,
                    height,
                    compression,
                    custom_bpp,
                    optimize,
                    force,
                    save_path.clone(),
                    progress_tx,
                    export_cancel,
                ), || stopped.store(true, Ordering::Release)).await
            });

            loop {
                while let Ok((rendered, total)) = progress_rx.try_recv() {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(ui) = this.export.as_mut() {
                            ui.rendered = rendered;
                            ui.total_frames = total;
                            ui.phase = ExportPhase::Rendering;
                        }
                        cx.notify();
                    });
                }
                if progress_rx.is_disconnected() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }

            match export.await {
                Ok(Ok(path)) => {
                    tracing::info!(path = %path.display(), "editor export completed");
                    if destination == ExportDestination::Clipboard {
                        let _ = this.update(cx, |this, cx| {
                            if let Some(ui) = this.export.as_mut() {
                                ui.copy_completed_export(path, |path| {
                                    let result = platform::copy_file_to_clipboard(path, cx);
                                    crate::app_sounds::play_notification();
                                    result
                                });
                            }
                            cx.notify();
                        });
                    } else {
                        if notify_file_save {
                            crate::app_sounds::play_notification();
                        }
                        let _ = this.update(cx, |this, cx| {
                            if let Some(ui) = this.export.as_mut() {
                                ui.phase = ExportPhase::Done;
                                ui.output_path = Some(path);
                            }
                            cx.notify();
                        });
                    }
                }
                Ok(Err(error)) => {
                    tracing::error!(error, "editor export failed");
                    let cancelled = !is_resource_stop(&error) && (error == "Export cancelled" || cancel.load(Ordering::Relaxed));
                    let _ = this.update(cx, |this, cx| {
                        if let Some(ui) = this.export.as_mut() {
                            if cancelled {
                                ui.phase = ExportPhase::Idle;
                            } else {
                                ui.phase = ExportPhase::Failed;
                                ui.error = Some(error);
                            }
                        }
                        cx.notify();
                    });
                }
                Err(_) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(ui) = this.export.as_mut() {
                            ui.phase = ExportPhase::Failed;
                            ui.error = Some("Export task failed".into());
                        }
                        cx.notify();
                    });
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_requested_export_close(window, cx)
            });
        }));
    }

    fn retry_clipboard_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        let Some(path) = ui.clipboard_retry_path().map(Path::to_path_buf) else {
            return;
        };
        ui.cancel = Arc::new(AtomicBool::new(false));
        ui.copy_completed_export(path, |path| {
            let result = platform::copy_file_to_clipboard(path, cx);
            crate::app_sounds::play_notification();
            result
        });
        cx.notify();
        self.finish_requested_export_close(window, cx);
    }

    fn subscribe_export_to_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sign_in = SignIn::global(cx);
        let subscriptions = [
            cx.observe(&sign_in, |_, _, cx| cx.notify()),
            cx.subscribe_in(&sign_in, window, |this, _, event, _, cx| {
                let Some(ui) = this.export.as_mut() else {
                    return;
                };
                match event {
                    SignInEvent::Finished {
                        outcome: SignInOutcome::SignedIn,
                        ..
                    } => {
                        ui.auth_rejected = false;
                        if ui.phase == ExportPhase::Failed {
                            ui.phase = ExportPhase::Idle;
                        }
                        ui.error = None;
                        ui.pick_default_organization();
                    }
                    SignInEvent::Finished {
                        origin: SignInOrigin::Editor,
                        outcome: SignInOutcome::Failed(error),
                    } => ui.error = Some(error.clone()),
                    SignInEvent::Finished { .. } => {}
                    SignInEvent::AccountRefreshed => ui.pick_default_organization(),
                }
                cx.notify();
            }),
        ];
        if let Some(ui) = self.export.as_mut() {
            ui.sign_in_subscriptions = Some(subscriptions);
        }
    }

    fn start_link_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let duration = self.total_duration();
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        if ui.phase.is_busy() || ui.close_requested {
            return;
        }

        let upgraded = store::auth_snapshot().is_upgraded();
        if !crate::direct_drive::enabled() && !upgraded && duration >= 300.0 {
            cx.open_url(crate::auth::PRICING_URL);
            return;
        }

        let estimate = ui
            .estimate
            .as_ref()
            .map(|estimate| estimate.size_range_mb[1])
            .or_else(|| {
                ui.preview_stats
                    .as_ref()
                    .map(|stats| stats.estimated_size_mb)
            })
            .map(estimated_working_bytes)
            .unwrap_or(0);
        let format = ui.format;
        let cursor_only = ui.cursor_only;
        let fps = ui.fps;
        let (width, height) = ui.resolution.size();
        let compression = ui.compression;
        let custom_bpp = ui.custom_bpp;
        let optimize = ui.optimize_filesize;
        let force = ui.force_ffmpeg;
        let project_path = self.project_path.clone();
        let project = self.project.clone();
        let drive_folder = ui.drive_folder.as_ref().map(|choice| choice.id.clone());
        let organization_id = ui.organization_id.clone().or_else(|| {
            store::auth_snapshot()
                .organizations
                .first()
                .map(|org| org.id.clone())
        });
        ui.cancel = Arc::new(AtomicBool::new(false));
        let cancel = ui.cancel.clone();
        ui.phase = ExportPhase::Starting;
        ui.reuploading = !crate::direct_drive::enabled() && self.sharing.is_some();
        ui.error = None;
        ui.share_link = None;
        ui.upload_progress = 0.0;
        ui.rendered = 0;
        ui.total_frames = 0;
        cx.notify();

        ui.export_task = Some(cx.spawn_in(window, async move |this, cx| {
            let save_path = RecordingMeta::load_for_project(&project_path)
                .ok()
                .map(|meta| meta.output_path());
            if let Some(path) = save_path.as_ref()
                && let Some(parent) = path.parent()
            {
                let _ = std::fs::create_dir_all(parent);
            }

            let resources = ExportResources::new(vec![DiskBudget {
                path: save_path
                    .clone()
                    .unwrap_or_else(|| project_path.join("output")),
                description: "your recording drive",
                estimated_bytes: estimate,
            }]);
            if !confirm_export_resources(&this, cx, &resources, &cancel).await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_requested_export_close(window, cx)
                });
                return;
            }

            let _ = this.update(cx, |this, cx| {
                if let Some(ui) = this.export.as_mut() {
                    ui.phase = ExportPhase::Rendering;
                }
                cx.notify();
            });

            let (progress_tx, progress_rx) = flume::unbounded::<(u32, u32)>();
            let export_cancel = cancel.clone();
            let export_path = project_path.clone();
            let export = gpui_tokio::Tokio::spawn(cx, async move {
                let stopped = export_cancel.clone();
                resources
                    .supervise(
                        run_export(
                            export_path,
                            project,
                            format,
                            cursor_only,
                            fps,
                            width,
                            height,
                            compression,
                            custom_bpp,
                            optimize,
                            force,
                            save_path.clone(),
                            progress_tx,
                            export_cancel,
                        ),
                        || stopped.store(true, Ordering::Release),
                    )
                    .await
            });

            loop {
                while let Ok((rendered, total)) = progress_rx.try_recv() {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(ui) = this.export.as_mut() {
                            ui.rendered = rendered;
                            ui.total_frames = total;
                            ui.phase = ExportPhase::Rendering;
                        }
                        cx.notify();
                    });
                }
                if progress_rx.is_disconnected() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }

            match export.await {
                Ok(Ok(path)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(ui) = this.export.as_mut() {
                            ui.phase = ExportPhase::Uploading;
                            ui.output_path = Some(path);
                            ui.upload_progress = 0.0;
                        }
                        cx.notify();
                    });

                    let (upload_tx, upload_rx) = flume::unbounded::<f64>();
                    let upload_cancel = cancel.clone();
                    let upload_project = project_path.clone();
                    let upload = gpui_tokio::Tokio::spawn(cx, async move {
                        if crate::direct_drive::enabled() {
                            return crate::direct_drive::upload_export(
                                &upload_project,
                                drive_folder.as_deref(),
                                |progress| {
                                    let _ = upload_tx.send(progress);
                                },
                                &upload_cancel,
                            )
                            .await;
                        }
                        crate::upload::upload_exported_video(
                            upload_project,
                            organization_id,
                            |progress| {
                                let _ = upload_tx.send(progress);
                            },
                            upload_cancel,
                        )
                        .await
                    });

                    loop {
                        while let Ok(progress) = upload_rx.try_recv() {
                            let _ = this.update(cx, |this, cx| {
                                if let Some(ui) = this.export.as_mut() {
                                    ui.upload_progress = progress as f32;
                                    ui.phase = ExportPhase::Uploading;
                                }
                                cx.notify();
                            });
                        }
                        if upload_rx.is_disconnected() {
                            break;
                        }
                        cx.background_executor()
                            .timer(Duration::from_millis(50))
                            .await;
                    }

                    match upload.await {
                        Ok(Ok(crate::upload::UploadResult::Success(link))) => {
                            let _ = this.update(cx, |this, cx| {
                                if let Ok(meta) =
                                    RecordingMeta::load_for_project(&this.project_path)
                                {
                                    this.sharing = meta.sharing;
                                }
                                if let Some(ui) = this.export.as_mut() {
                                    ui.complete_share_export(link, |link| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            link,
                                        ));
                                        crate::app_sounds::play_notification();
                                    });
                                }
                                cx.notify();
                            });
                        }
                        Ok(Ok(crate::upload::UploadResult::NotAuthenticated)) => {
                            let _ = this.update(cx, |this, _| {
                                if let Some(ui) = this.export.as_mut() {
                                    ui.auth_rejected = true;
                                }
                            });
                            show_upload_failure(
                                &this,
                                cx,
                                "You need to sign in to share recordings",
                                true,
                            );
                        }
                        Ok(Ok(crate::upload::UploadResult::UpgradeRequired)) => {
                            let _ = this.update(cx, |this, cx| {
                                if let Some(ui) = this.export.as_mut() {
                                    ui.phase = ExportPhase::Idle;
                                }
                                cx.notify();
                            });
                            let _ = this.update(cx, |_, cx| {
                                cx.open_url(crate::auth::PRICING_URL);
                            });
                            platform::alert_dialog(
                                "Upgrade required",
                                "This feature requires an upgraded plan",
                            );
                        }
                        Ok(Err(error)) => {
                            if error == "Export cancelled" || cancel.load(Ordering::Relaxed) {
                                let _ = this.update(cx, |this, cx| {
                                    if let Some(ui) = this.export.as_mut() {
                                        ui.phase = ExportPhase::Idle;
                                    }
                                    cx.notify();
                                });
                            } else {
                                show_upload_failure(&this, cx, &error, true);
                            }
                        }
                        Err(_) => {
                            show_upload_failure(&this, cx, "Failed to upload recording", true);
                        }
                    }
                }
                Ok(Err(error)) => {
                    let cancelled = !is_resource_stop(&error)
                        && (error == "Export cancelled" || cancel.load(Ordering::Relaxed));
                    let _ = this.update(cx, |this, cx| {
                        if let Some(ui) = this.export.as_mut() {
                            if cancelled {
                                ui.phase = ExportPhase::Idle;
                            } else {
                                ui.phase = ExportPhase::Failed;
                                ui.error = Some(error.clone());
                            }
                        }
                        cx.notify();
                    });
                    if !cancelled {
                        platform::alert_dialog("Export failed", &error);
                    }
                }
                Err(_) => {
                    show_upload_failure(&this, cx, "Export task failed", false);
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_requested_export_close(window, cx)
            });
        }));
    }

    fn quick_setting_value(&self, ui: &ExportUi, setting: QuickSetting) -> Option<String> {
        match setting {
            QuickSetting::Format => Some(ui.format.label().to_string()),
            QuickSetting::Resolution => Some(ui.resolution.label().to_string()),
            QuickSetting::Fps => Some(format!("{} FPS", ui.fps)),
            QuickSetting::Quality => {
                if ui.format != ExportFormatKind::Mp4 || ui.cursor_only {
                    None
                } else if ui.is_custom_bpp() {
                    Some("Eigen".to_string())
                } else {
                    COMPRESSION_PRESETS
                        .iter()
                        .find(|(value, _)| matches_compression(ui.compression, *value))
                        .map(|(_, label)| label.to_string())
                }
            }
        }
    }

    fn quick_setting_enabled(&self, ui: &ExportUi, setting: QuickSetting) -> bool {
        let locked = has_transparent_background(&self.project) || ui.cursor_only;
        match setting {
            QuickSetting::Format => !locked,
            QuickSetting::Quality => self.quick_setting_value(ui, setting).is_some(),
            QuickSetting::Resolution | QuickSetting::Fps => true,
        }
    }

    fn quick_setting_items(&self, ui: &ExportUi, setting: QuickSetting) -> Vec<ui::MenuItem> {
        match setting {
            QuickSetting::Format => [ExportFormatKind::Mp4, ExportFormatKind::Gif]
                .iter()
                .map(|format| ui::MenuItem::new(format.label(), ui.format == *format))
                .collect(),
            QuickSetting::Resolution => self
                .export_resolutions(ui)
                .iter()
                .map(|res| ui::MenuItem::new(res.label(), ui.resolution == *res))
                .collect(),
            QuickSetting::Fps => self
                .export_fps_options(ui)
                .iter()
                .map(|fps| ui::MenuItem::new(format!("{fps} FPS"), ui.fps == *fps))
                .collect(),
            QuickSetting::Quality => COMPRESSION_PRESETS
                .iter()
                .map(|(value, label)| {
                    ui::MenuItem::new(
                        *label,
                        !ui.is_custom_bpp() && matches_compression(ui.compression, *value),
                    )
                })
                .collect(),
        }
    }

    fn open_quick_setting(
        &mut self,
        setting: QuickSetting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ui) = self.export.as_ref() else {
            return;
        };
        if ui.phase.shows_progress() || !self.quick_setting_enabled(ui, setting) {
            return;
        }
        if ui
            .quick_menu
            .as_ref()
            .is_some_and(|menu| menu.setting == setting)
        {
            if let Some(ui) = self.export.as_mut() {
                ui.quick_menu = None;
            }
            cx.notify();
            return;
        }
        let Some(bounds) = ui.chip_bounds[setting.index()].get() else {
            return;
        };
        let items = self.quick_setting_items(ui, setting);
        let focus = self.focus_handle_for_menu();
        window.focus(&focus, cx);
        if let Some(ui) = self.export.as_mut() {
            ui.quick_menu = Some(QuickMenu {
                setting,
                state: ui::MenuState::anchored(bounds, &items),
            });
        }
        cx.notify();
    }

    fn choose_quick_setting(
        &mut self,
        setting: QuickSetting,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let resolutions = self
            .export
            .as_ref()
            .map(|ui| self.export_resolutions(ui))
            .unwrap_or_default();
        let fps_options = self
            .export
            .as_ref()
            .map(|ui| self.export_fps_options(ui).to_vec())
            .unwrap_or_default();
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        ui.quick_menu = None;
        match setting {
            QuickSetting::Format => {
                let Some(format) = [ExportFormatKind::Mp4, ExportFormatKind::Gif]
                    .get(index)
                    .copied()
                else {
                    return;
                };
                ui.format = format;
                if format == ExportFormatKind::Gif {
                    if ui.destination == ExportDestination::Link {
                        ui.destination = ExportDestination::File;
                    }
                    if ui.resolution == ExportResolution::P4k {
                        ui.resolution = ExportResolution::P1080;
                    }
                }
                self.normalize_loaded_export_fps();
            }
            QuickSetting::Resolution => {
                let Some(resolution) = resolutions.get(index).copied() else {
                    return;
                };
                ui.resolution = resolution;
            }
            QuickSetting::Fps => {
                let Some(fps) = fps_options.get(index).copied() else {
                    return;
                };
                ui.fps = fps;
            }
            QuickSetting::Quality => {
                let Some((compression, _)) = COMPRESSION_PRESETS.get(index).copied() else {
                    return;
                };
                ui.compression = compression;
                ui.custom_bpp = None;
            }
        }
        if let Some(ui) = self.export.as_mut() {
            ui.persist();
        }
        self.refresh_export_preview(window, cx);
        cx.notify();
    }

    pub(crate) fn export_quick_menu_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.export.as_mut().and_then(|ui| ui.quick_menu.as_mut()) else {
            if key == "escape"
                && self
                    .export
                    .as_ref()
                    .is_some_and(|ui| ui.folder_picker.is_some())
            {
                self.close_folder_picker(cx);
                return true;
            }
            if key == "escape" && self.export.as_ref().is_some_and(|ui| ui.settings_open) {
                self.close_export_settings(cx);
                return true;
            }
            return false;
        };
        let setting = menu.setting;
        match menu.state.on_key(key) {
            ui::MenuKey::Moved => {
                cx.notify();
                true
            }
            ui::MenuKey::Commit(index) => {
                self.choose_quick_setting(setting, index, window, cx);
                true
            }
            ui::MenuKey::Dismiss => {
                if let Some(ui) = self.export.as_mut() {
                    ui.quick_menu = None;
                }
                cx.notify();
                true
            }
            ui::MenuKey::Ignored => false,
        }
    }

    pub(crate) fn render_export_quick_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let ui = self.export.as_ref()?;
        let menu = ui.quick_menu.as_ref()?;
        let setting = menu.setting;
        let items = self.quick_setting_items(ui, setting);
        Some(
            ui::Menu::editor(&self.theme, "export-quick-menu", items, &menu.state)
                .min_width(px(150.))
                .on_select(cx.listener(move |this, index: &usize, window, cx| {
                    this.choose_quick_setting(setting, *index, window, cx);
                }))
                .on_dismiss(cx.listener(|this, _, _window, cx| {
                    if let Some(ui) = this.export.as_mut() {
                        ui.quick_menu = None;
                    }
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    fn render_quick_setting_chip(
        &self,
        ui: &ExportUi,
        setting: QuickSetting,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let editor = self.theme.editor;
        let enabled = self.quick_setting_enabled(ui, setting);
        let open = ui
            .quick_menu
            .as_ref()
            .is_some_and(|menu| menu.setting == setting);
        let bounds = ui.chip_bounds[setting.index()].clone();
        let value = self
            .quick_setting_value(ui, setting)
            .unwrap_or_else(|| "—".to_string());

        div()
            .id(SharedString::from(format!(
                "export-chip-{}",
                setting.index()
            )))
            .tab_index(0)
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .justify_center()
            .gap(px(2.))
            .h(px(50.))
            .px(px(10.))
            .rounded(px(9.))
            .bg(Hsla::from(editor.card_2))
            .border_1()
            .border_color(if open {
                Hsla::from(editor.accent)
            } else {
                Hsla::from(editor.line)
            })
            .when(!enabled, |this| this.opacity(0.5))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(Hsla::from(editor.ctl_hover)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_quick_setting(setting, window, cx);
                    }))
            })
            .child(
                gpui::canvas(move |value, _, _| bounds.set(Some(value)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(Hsla::from(editor.text_2))
                    .child(setting.label()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(4.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(value),
                    )
                    .when(enabled, |this| {
                        this.child(
                            svg()
                                .path("icons/chevron-down.svg")
                                .size(px(11.))
                                .flex_shrink_0()
                                .text_color(Hsla::from(editor.text_2)),
                        )
                    }),
            )
    }

    fn render_export_overview(&self, ui: &ExportUi, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.theme.editor;
        let drive = ui.destination == ExportDestination::Link && crate::direct_drive::enabled();

        let mut chips = div().flex().flex_row().gap(px(6.));
        for setting in QuickSetting::ALL {
            chips = chips.child(self.render_quick_setting_chip(ui, setting, cx));
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .p(px(12.))
            .gap(px(8.))
            .child(chips)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .when(drive, |this| {
                        this.child(self.render_drive_folder_button(ui, cx))
                    })
                    .when(!drive, |this| this.child(div().flex_1()))
                    .child(
                        div()
                            .id("export-all-settings")
                            .tab_index(0)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .h(px(26.))
                            .px(px(10.))
                            .rounded(px(7.))
                            .bg(if ui.settings_open {
                                Hsla::from(editor.ctl_hover)
                            } else {
                                Hsla::from(editor.ctl)
                            })
                            .cursor_pointer()
                            .hover(move |style| style.bg(Hsla::from(editor.ctl_hover)))
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(
                                svg()
                                    .path("icons/sliders-horizontal.svg")
                                    .size(px(13.))
                                    .text_color(Hsla::from(editor.text_1)),
                            )
                            .child("Alle instellingen")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                if let Some(ui) = this.export.as_mut() {
                                    ui.quick_menu = None;
                                    ui.settings_open = !ui.settings_open;
                                }
                                cx.notify();
                            })),
                    ),
            )
    }

    fn drive_folder_label(&self, ui: &ExportUi) -> String {
        match &ui.drive_folder {
            Some(choice) => {
                let parts: Vec<&str> = choice.path.split(" › ").collect();
                if parts.len() > 2 {
                    parts[parts.len() - 2..].join(" › ")
                } else {
                    choice.path.clone()
                }
            }
            None => "Kiekes-opnames".to_string(),
        }
    }

    fn load_team_destination(&mut self, cx: &mut Context<Self>) {
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        if !crate::direct_drive::enabled()
            || !cap_direct_drive::is_connected()
            || !cap_direct_drive::can_browse_folders()
            || !cap_direct_drive::has_team_folder()
            || ui.team_task.is_some()
        {
            return;
        }
        ui.team_loading = true;
        ui.team_task = Some(cx.spawn(async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn(cx, cap_direct_drive::team_destination())
                .await
                .ok();
            let _ = this.update(cx, |this, cx| {
                let Some(ui) = this.export.as_mut() else {
                    return;
                };
                ui.team_loading = false;
                match result {
                    Some(Ok(Some(team))) => {
                        let choice = DriveChoice {
                            id: team.folder.id.clone(),
                            path: team.path.join(" › "),
                        };
                        if ui.drive_folder.is_none() {
                            ui.drive_folder = Some(choice.clone());
                            ui.persist();
                        }
                        ui.team_folder = Some(choice);
                    }
                    Some(Ok(None)) => {}
                    Some(Err(error)) => ui.team_error = Some(error),
                    None => ui.team_error = Some("Could not reach Google Drive".into()),
                }
                cx.notify();
            });
        }));
    }

    fn open_folder_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = self.export.as_ref() else {
            return;
        };
        if ui.phase.shows_progress() {
            return;
        }
        let search_input = cx.new(|cx| {
            let mut input = ui::TextInputState::single_line(window, cx);
            input.set_placeholder("Zoek een map in Drive");
            input
        });
        let name_input = cx.new(|cx| {
            let mut input = ui::TextInputState::single_line(window, cx);
            input.set_placeholder("Naam van de nieuwe map");
            input
        });
        let search_events = cx.subscribe(&search_input, |this, input, event, cx| {
            if matches!(event, ui::TextInputEvent::Changed) {
                let text = input.read(cx).text().to_string();
                this.search_drive_folders(text, cx);
            }
        });
        let name_events = cx.subscribe(&name_input, |this, input, event, cx| match event {
            ui::TextInputEvent::Confirmed => {
                let name = input.read(cx).text().to_string();
                this.create_drive_folder(name, cx);
            }
            ui::TextInputEvent::Cancelled => {
                if let Some(picker) = this
                    .export
                    .as_mut()
                    .and_then(|ui| ui.folder_picker.as_mut())
                {
                    picker.creating = false;
                }
                cx.notify();
            }
            _ => {}
        });
        if let Some(ui) = self.export.as_mut() {
            ui.settings_open = false;
            ui.quick_menu = None;
            ui.folder_picker = Some(FolderPicker {
                trail: Vec::new(),
                entries: FolderEntries::Loading,
                selected: None,
                creating: false,
                searching: false,
                search_input,
                name_input,
                task: None,
                generation: 0,
                _subscriptions: [search_events, name_events],
            });
        }
        self.load_team_destination(cx);
        self.load_drive_folders(cx);
        cx.notify();
    }

    fn close_folder_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(ui) = self.export.as_mut() {
            ui.folder_picker = None;
        }
        cx.notify();
    }

    fn load_drive_folders(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self
            .export
            .as_mut()
            .and_then(|ui| ui.folder_picker.as_mut())
        else {
            return;
        };
        if !cap_direct_drive::is_connected() || !cap_direct_drive::can_browse_folders() {
            picker.entries = FolderEntries::NeedsReconnect;
            return;
        }
        picker.generation += 1;
        let generation = picker.generation;
        picker.entries = FolderEntries::Loading;
        picker.selected = None;
        let parent = picker.trail.last().map(|folder| folder.id.clone());
        picker.task = Some(cx.spawn(async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                match parent {
                    None => {
                        let mut entries = vec![cap_direct_drive::DriveFolder {
                            id: "root".into(),
                            name: "Mijn Drive".into(),
                        }];
                        entries.extend(
                            cap_direct_drive::list_folders(
                                cap_direct_drive::FolderParent::SharedDrives,
                            )
                            .await?,
                        );
                        Ok(entries)
                    }
                    Some(id) if id == "root" => {
                        cap_direct_drive::list_folders(cap_direct_drive::FolderParent::MyDrive)
                            .await
                    }
                    Some(id) => {
                        cap_direct_drive::list_folders(cap_direct_drive::FolderParent::Folder(id))
                            .await
                    }
                }
            })
            .await
            .ok();
            this.update(cx, |this, cx| {
                this.finish_drive_folders(generation, result, cx)
            })
            .ok();
        }));
    }

    fn search_drive_folders(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(picker) = self
            .export
            .as_mut()
            .and_then(|ui| ui.folder_picker.as_mut())
        else {
            return;
        };
        if text.trim().is_empty() {
            picker.searching = false;
            self.load_drive_folders(cx);
            cx.notify();
            return;
        }
        picker.searching = true;
        picker.creating = false;
        picker.generation += 1;
        let generation = picker.generation;
        picker.entries = FolderEntries::Loading;
        picker.selected = None;
        picker.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                cap_direct_drive::search_folders(&text).await
            })
            .await
            .ok();
            this.update(cx, |this, cx| {
                this.finish_drive_folders(generation, result, cx)
            })
            .ok();
        }));
        cx.notify();
    }

    fn finish_drive_folders(
        &mut self,
        generation: u64,
        result: Option<Result<Vec<cap_direct_drive::DriveFolder>, String>>,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self
            .export
            .as_mut()
            .and_then(|ui| ui.folder_picker.as_mut())
        else {
            return;
        };
        if picker.generation != generation {
            return;
        }
        picker.entries = match result {
            Some(Ok(folders)) => FolderEntries::Loaded(folders),
            Some(Err(error)) => FolderEntries::Failed(error),
            None => FolderEntries::Failed("Could not reach Google Drive".into()),
        };
        cx.notify();
    }

    fn create_drive_folder(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(picker) = self
            .export
            .as_mut()
            .and_then(|ui| ui.folder_picker.as_mut())
        else {
            return;
        };
        let name = name.trim().to_string();
        let Some(parent) = picker.trail.last().map(|folder| folder.id.clone()) else {
            return;
        };
        if name.is_empty() || picker.searching {
            return;
        }
        picker.generation += 1;
        let generation = picker.generation;
        picker.task = Some(cx.spawn(async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                cap_direct_drive::create_folder(&parent, &name).await
            })
            .await
            .ok();
            this.update(cx, |this, cx| {
                let Some(picker) = this
                    .export
                    .as_mut()
                    .and_then(|ui| ui.folder_picker.as_mut())
                else {
                    return;
                };
                if picker.generation != generation {
                    return;
                }
                match result {
                    Some(Ok(folder)) => {
                        picker.creating = false;
                        picker
                            .name_input
                            .update(cx, |input, cx| input.set_text("", cx));
                        if let FolderEntries::Loaded(entries) = &mut picker.entries {
                            entries.insert(0, folder.clone());
                        } else {
                            picker.entries = FolderEntries::Loaded(vec![folder.clone()]);
                        }
                        picker.selected = Some(folder);
                    }
                    Some(Err(error)) => picker.entries = FolderEntries::Failed(error),
                    None => {
                        picker.entries =
                            FolderEntries::Failed("Could not reach Google Drive".into())
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_drive_folder(&mut self, folder: cap_direct_drive::DriveFolder, cx: &mut Context<Self>) {
        let Some(picker) = self
            .export
            .as_mut()
            .and_then(|ui| ui.folder_picker.as_mut())
        else {
            return;
        };
        if picker.searching {
            picker.searching = false;
            picker.trail.clear();
            picker
                .search_input
                .update(cx, |input, cx| input.set_text("", cx));
        }
        picker.creating = false;
        picker.trail.push(folder);
        self.load_drive_folders(cx);
        cx.notify();
    }

    fn drive_trail_back_to(&mut self, depth: usize, cx: &mut Context<Self>) {
        let Some(picker) = self
            .export
            .as_mut()
            .and_then(|ui| ui.folder_picker.as_mut())
        else {
            return;
        };
        picker.trail.truncate(depth);
        picker.creating = false;
        self.load_drive_folders(cx);
        cx.notify();
    }

    fn choose_drive_folder(&mut self, choice: DriveChoice, cx: &mut Context<Self>) {
        if let Some(ui) = self.export.as_mut() {
            ui.drive_folder = Some(choice);
            ui.folder_picker = None;
            ui.persist();
        }
        cx.notify();
    }

    fn picker_target(picker: &FolderPicker) -> Option<DriveChoice> {
        let mut names: Vec<String> = if picker.searching {
            Vec::new()
        } else {
            picker
                .trail
                .iter()
                .map(|folder| folder.name.clone())
                .collect()
        };
        let folder = match &picker.selected {
            Some(folder) => {
                names.push(folder.name.clone());
                folder.clone()
            }
            None if !picker.searching => picker.trail.last()?.clone(),
            None => return None,
        };
        Some(DriveChoice {
            id: folder.id,
            path: names.join(" › "),
        })
    }

    fn render_drive_folder_button(
        &self,
        ui: &ExportUi,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let editor = self.theme.editor;
        let open = ui.folder_picker.is_some();
        div()
            .id("export-drive-folder")
            .tab_index(0)
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .pl(px(6.))
            .pr(px(8.))
            .rounded(px(7.))
            .bg(if open {
                Hsla::from(editor.ctl_hover)
            } else {
                Hsla::from(editor.ctl)
            })
            .cursor_pointer()
            .hover(move |style| style.bg(Hsla::from(editor.ctl_hover)))
            .text_size(px(12.))
            .child(
                svg()
                    .path("icons/folder.svg")
                    .size(px(14.))
                    .flex_shrink_0()
                    .text_color(Hsla::from(editor.text_2)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(self.drive_folder_label(ui)),
            )
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .size(px(11.))
                    .flex_shrink_0()
                    .text_color(Hsla::from(editor.text_2)),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                if this
                    .export
                    .as_ref()
                    .is_some_and(|ui| ui.folder_picker.is_some())
                {
                    this.close_folder_picker(cx);
                } else {
                    this.open_folder_picker(window, cx);
                }
            }))
    }

    pub(crate) fn render_drive_folder_picker(
        &self,
        bottom: f32,
        max_height: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let ui = self.export.as_ref()?;
        let picker = ui.folder_picker.as_ref()?;
        let theme = self.theme;
        let editor = theme.editor;
        let mut scrim: Hsla = gpui::black();
        scrim.a = if theme.is_dark() { 0.25 } else { 0.08 };
        let target = Self::picker_target(picker);
        let can_create = !picker.searching && !picker.trail.is_empty();
        let save_label = match &target {
            Some(choice) => format!(
                "Opslaan in {}",
                choice.path.rsplit(" › ").next().unwrap_or(&choice.path)
            ),
            None => "Kies een map".to_string(),
        };

        let row = |id: SharedString, folder: cap_direct_drive::DriveFolder, selected: bool| {
            let select_folder = folder.clone();
            let open_folder = folder.clone();
            div()
                .flex()
                .flex_row()
                .items_center()
                .h(px(40.))
                .pl(px(8.))
                .pr(px(4.))
                .rounded(px(8.))
                .when(selected, |this| {
                    this.bg(crate::theme::Theme::with_alpha(editor.accent, 0.16))
                })
                .child(
                    div()
                        .id(id.clone())
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .items_center()
                        .gap(px(10.))
                        .cursor_pointer()
                        .text_size(px(13.))
                        .child(
                            svg()
                                .path("icons/folder.svg")
                                .size(px(16.))
                                .flex_shrink_0()
                                .text_color(if selected {
                                    Hsla::from(editor.accent)
                                } else {
                                    Hsla::from(editor.text_2)
                                }),
                        )
                        .child(div().min_w_0().truncate().child(folder.name.clone()))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            if let Some(picker) = this
                                .export
                                .as_mut()
                                .and_then(|ui| ui.folder_picker.as_mut())
                            {
                                picker.selected = if picker
                                    .selected
                                    .as_ref()
                                    .is_some_and(|current| current.id == select_folder.id)
                                {
                                    None
                                } else {
                                    Some(select_folder.clone())
                                };
                            }
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-open")))
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(move |style| style.bg(Hsla::from(editor.ctl)))
                        .child(
                            svg()
                                .path("icons/chevron-right.svg")
                                .size(px(14.))
                                .text_color(Hsla::from(editor.text_2)),
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.open_drive_folder(open_folder.clone(), cx);
                        })),
                )
        };

        let list = match &picker.entries {
            FolderEntries::Loading => div()
                .p(px(12.))
                .text_size(px(12.))
                .text_color(Hsla::from(editor.text_2))
                .child("Mappen laden…")
                .into_any_element(),
            FolderEntries::Failed(error) => div()
                .p(px(12.))
                .text_size(px(12.))
                .text_color(Hsla::from(theme.red_9))
                .child(error.clone())
                .into_any_element(),
            FolderEntries::NeedsReconnect => div()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(editor.text_2))
                        .child("Koppel Google Drive opnieuw om mappen te kunnen kiezen."),
                )
                .child(
                    div()
                        .id("drive-reconnect")
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(Hsla::from(editor.accent))
                        .cursor_pointer()
                        .child("Open Instellingen")
                        .on_click(|_, _, cx| {
                            crate::app_windows::open_settings(
                                crate::settings_window::Page::Integrations,
                                cx,
                            )
                        }),
                )
                .into_any_element(),
            FolderEntries::Loaded(folders) if folders.is_empty() => div()
                .p(px(12.))
                .text_size(px(12.))
                .text_color(Hsla::from(editor.text_2))
                .child(if picker.searching {
                    "Geen mappen gevonden"
                } else {
                    "Deze map heeft nog geen submappen"
                })
                .into_any_element(),
            FolderEntries::Loaded(folders) => {
                let mut list = div().flex().flex_col();
                for (index, folder) in folders.iter().enumerate() {
                    let selected = picker
                        .selected
                        .as_ref()
                        .is_some_and(|current| current.id == folder.id);
                    list = list.child(row(
                        SharedString::from(format!("drive-folder-{index}")),
                        folder.clone(),
                        selected,
                    ));
                }
                list.into_any_element()
            }
        };

        let mut crumbs = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .text_size(px(11.))
            .text_color(Hsla::from(editor.text_2))
            .child(
                div()
                    .id("drive-crumb-top")
                    .cursor_pointer()
                    .when(picker.trail.is_empty(), |this| {
                        this.font_weight(FontWeight::SEMIBOLD)
                            .text_color(Hsla::from(editor.text_1))
                    })
                    .child("Drive")
                    .on_click(cx.listener(|this, _, _window, cx| this.drive_trail_back_to(0, cx))),
            );
        for (index, folder) in picker.trail.iter().enumerate() {
            let last = index + 1 == picker.trail.len();
            crumbs = crumbs.child("›").child(
                div()
                    .id(SharedString::from(format!("drive-crumb-{index}")))
                    .cursor_pointer()
                    .when(last, |this| {
                        this.font_weight(FontWeight::SEMIBOLD)
                            .text_color(Hsla::from(editor.text_1))
                    })
                    .child(folder.name.clone())
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.drive_trail_back_to(index + 1, cx)
                    })),
            );
        }

        let team = ui.team_folder.clone();
        let team_selected = team.as_ref().is_some_and(|team| {
            ui.drive_folder
                .as_ref()
                .is_some_and(|current| current.id == team.id)
        });

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .id("drive-picker-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(scrim)
                        .occlude()
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.close_folder_picker(cx);
                        })),
                )
                .child(
                    div()
                        .id("drive-picker")
                        .absolute()
                        .right(px(8.))
                        .bottom(px(bottom))
                        .w(px(EXPORT_PANEL_WIDTH))
                        .h(px(max_height.clamp(200., 460.)))
                        .occlude()
                        .flex()
                        .flex_col()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(Hsla::from(editor.line))
                        .bg(Hsla::from(editor.card))
                        .shadow(vec![gpui::BoxShadow {
                            color: {
                                let mut color: Hsla = gpui::black();
                                color.a = if theme.is_dark() { 0.55 } else { 0.22 };
                                color
                            },
                            offset: gpui::point(px(0.), px(18.)),
                            blur_radius: px(50.),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                        .overflow_hidden()
                        .text_color(Hsla::from(editor.text_1))
                        .child(
                            div()
                                .flex_none()
                                .h(px(48.))
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .pl(px(16.))
                                .pr(px(12.))
                                .border_b_1()
                                .border_color(Hsla::from(editor.line))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Opslaan in Drive"),
                                )
                                .child(
                                    div()
                                        .id("drive-picker-close")
                                        .size(px(28.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(7.))
                                        .cursor_pointer()
                                        .hover(move |style| style.bg(Hsla::from(editor.ctl)))
                                        .child(
                                            svg()
                                                .path("icons/x.svg")
                                                .size(px(14.))
                                                .text_color(Hsla::from(editor.text_2)),
                                        )
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.close_folder_picker(cx);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px(px(16.))
                                .pt(px(12.))
                                .pb(px(8.))
                                .flex()
                                .flex_col()
                                .gap(px(10.))
                                .when_some(team, |this, team| {
                                    let choice = team.clone();
                                    this.child(
                                        div()
                                            .id("drive-team-folder")
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(10.))
                                            .px(px(12.))
                                            .py(px(10.))
                                            .rounded(px(10.))
                                            .border_1()
                                            .border_color(if team_selected {
                                                crate::theme::Theme::with_alpha(editor.accent, 0.5)
                                            } else {
                                                Hsla::from(editor.line)
                                            })
                                            .when(team_selected, |this| {
                                                this.bg(crate::theme::Theme::with_alpha(
                                                    editor.accent,
                                                    0.12,
                                                ))
                                            })
                                            .cursor_pointer()
                                            .hover(move |style| style.bg(Hsla::from(editor.ctl)))
                                            .child(
                                                div()
                                                    .size(px(30.))
                                                    .flex_none()
                                                    .rounded(px(8.))
                                                    .bg(crate::theme::Theme::with_alpha(
                                                        editor.accent,
                                                        0.2,
                                                    ))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        svg()
                                                            .path("icons/user-round.svg")
                                                            .size(px(16.))
                                                            .text_color(Hsla::from(editor.accent)),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap(px(2.))
                                                    .child(
                                                        div()
                                                            .text_size(px(13.))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .child("Mijn teammap"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(11.))
                                                            .text_color(Hsla::from(editor.text_2))
                                                            .truncate()
                                                            .child(team.path.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .px(px(7.))
                                                    .py(px(2.))
                                                    .rounded_full()
                                                    .bg(Hsla::from(editor.ctl))
                                                    .text_size(px(10.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(Hsla::from(editor.text_2))
                                                    .child("Standaard"),
                                            )
                                            .on_click(cx.listener(move |this, _, _window, cx| {
                                                this.choose_drive_folder(choice.clone(), cx);
                                            })),
                                    )
                                })
                                .when(ui.team_loading, |this| {
                                    this.child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(Hsla::from(editor.text_2))
                                            .child("Teammap laden…"),
                                    )
                                })
                                .when_some(ui.team_error.clone(), |this, error| {
                                    this.child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(Hsla::from(theme.red_9))
                                            .child(format!("Teammap niet beschikbaar: {error}")),
                                    )
                                })
                                .child(ui::TextInput::plain(
                                    &theme,
                                    "drive-folder-search",
                                    &picker.search_input,
                                ))
                                .when(!picker.searching, |this| this.child(crumbs)),
                        )
                        .child(
                            div()
                                .id("drive-folder-list")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .px(px(8.))
                                .when(picker.creating, |this| {
                                    this.child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(8.))
                                            .mb(px(2.))
                                            .p(px(6.))
                                            .rounded(px(8.))
                                            .border_1()
                                            .border_color(crate::theme::Theme::with_alpha(
                                                editor.accent,
                                                0.45,
                                            ))
                                            .bg(crate::theme::Theme::with_alpha(
                                                editor.accent,
                                                0.12,
                                            ))
                                            .child(div().flex_1().min_w_0().child(
                                                ui::TextInput::plain(
                                                    &theme,
                                                    "drive-new-folder-name",
                                                    &picker.name_input,
                                                ),
                                            ))
                                            .child(
                                                div()
                                                    .id("drive-new-folder-create")
                                                    .h(px(26.))
                                                    .px(px(10.))
                                                    .flex()
                                                    .items_center()
                                                    .rounded(px(6.))
                                                    .bg(Hsla::from(editor.accent))
                                                    .cursor_pointer()
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(gpui::white())
                                                    .child("Maken")
                                                    .on_click(cx.listener(
                                                        |this, _, _window, cx| {
                                                            let name = this
                                                                .export
                                                                .as_ref()
                                                                .and_then(|ui| {
                                                                    ui.folder_picker.as_ref()
                                                                })
                                                                .map(|picker| {
                                                                    picker
                                                                        .name_input
                                                                        .read(cx)
                                                                        .text()
                                                                        .to_string()
                                                                })
                                                                .unwrap_or_default();
                                                            this.create_drive_folder(name, cx);
                                                        },
                                                    )),
                                            ),
                                    )
                                })
                                .child(list),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap(px(8.))
                                .px(px(16.))
                                .pt(px(10.))
                                .pb(px(12.))
                                .border_t_1()
                                .border_color(Hsla::from(editor.line))
                                .child(
                                    div()
                                        .id("drive-new-folder")
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.))
                                        .h(px(32.))
                                        .px(px(10.))
                                        .rounded(px(8.))
                                        .bg(Hsla::from(editor.ctl))
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .when(!can_create, |this| this.opacity(0.5))
                                        .when(can_create, |this| {
                                            this.cursor_pointer()
                                                .hover(move |style| {
                                                    style.bg(Hsla::from(editor.ctl_hover))
                                                })
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    if let Some(picker) = this
                                                        .export
                                                        .as_mut()
                                                        .and_then(|ui| ui.folder_picker.as_mut())
                                                    {
                                                        picker.creating = true;
                                                        let input = picker.name_input.clone();
                                                        input.update(cx, |input, cx| {
                                                            input.focus_and_select_all(window, cx)
                                                        });
                                                    }
                                                    cx.notify();
                                                }))
                                        })
                                        .child(
                                            svg()
                                                .path("icons/plus.svg")
                                                .size(px(13.))
                                                .text_color(Hsla::from(editor.text_1)),
                                        )
                                        .child("Nieuwe map"),
                                )
                                .child(
                                    div()
                                        .id("drive-folder-save")
                                        .min_w_0()
                                        .h(px(32.))
                                        .px(px(14.))
                                        .flex()
                                        .items_center()
                                        .rounded(px(8.))
                                        .bg(Hsla::from(editor.accent))
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(gpui::white())
                                        .child(div().truncate().child(save_label))
                                        .when(target.is_none(), |this| this.opacity(0.5))
                                        .when_some(target, |this, target| {
                                            this.cursor_pointer()
                                                .hover(move |style| {
                                                    style.bg(Hsla::from(editor.accent_2))
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, _window, cx| {
                                                        this.choose_drive_folder(
                                                            target.clone(),
                                                            cx,
                                                        );
                                                    },
                                                ))
                                        }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn close_export_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(ui) = self.export.as_mut() {
            ui.settings_open = false;
        }
        cx.notify();
    }

    fn restore_export_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        ui.format = ExportFormatKind::Mp4;
        ui.resolution = ExportResolution::P720;
        ui.fps = 30;
        ui.compression = ExportCompression::Maximum;
        ui.custom_bpp = None;
        ui.optimize_filesize = false;
        ui.cursor_only = false;
        ui.force_ffmpeg = false;
        if has_transparent_background(&self.project) {
            ui.format = ExportFormatKind::Gif;
            if ui.destination == ExportDestination::Link {
                ui.destination = ExportDestination::File;
            }
        }
        self.normalize_loaded_export_fps();
        if let Some(ui) = self.export.as_mut() {
            ui.persist();
        }
        self.refresh_export_preview(window, cx);
        cx.notify();
    }

    /// "Alle instellingen": every export setting in a card that opens above
    /// the export panel. `bottom` is the panel's top edge from the window
    /// bottom and `max_height` what fits between the header and it.
    pub(crate) fn render_export_settings_popover(
        &self,
        bottom: f32,
        max_height: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let ui = self.export.as_ref()?;
        if !ui.settings_open || ui.phase.shows_progress() {
            return None;
        }
        let theme = self.theme;
        let editor = theme.editor;
        let transparent = has_transparent_background(&self.project);
        let link_disabled = transparent || ui.cursor_only;
        let format_locked = transparent || ui.cursor_only;
        let resolutions = self.export_resolutions(ui);
        let fps_options = self.export_fps_options(ui);
        let mut scrim: Hsla = gpui::black();
        scrim.a = if theme.is_dark() { 0.25 } else { 0.08 };

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .id("export-settings-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(scrim)
                        .occlude()
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.close_export_settings(cx);
                        })),
                )
                .child(
                    div()
                        .id("export-settings-popover")
                        .absolute()
                        .right(px(8.))
                        .bottom(px(bottom))
                        .w(px(EXPORT_PANEL_WIDTH))
                        .max_h(px(max_height.clamp(160., 520.)))
                        .occlude()
                        .flex()
                        .flex_col()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(Hsla::from(editor.line))
                        .bg(Hsla::from(editor.card))
                        .shadow(vec![gpui::BoxShadow {
                            color: {
                                let mut color: Hsla = gpui::black();
                                color.a = if theme.is_dark() { 0.55 } else { 0.22 };
                                color
                            },
                            offset: gpui::point(px(0.), px(18.)),
                            blur_radius: px(50.),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                        .overflow_hidden()
                        .text_color(Hsla::from(editor.text_1))
                        .child(
                            div()
                                .flex_none()
                                .h(px(48.))
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .pl(px(16.))
                                .pr(px(12.))
                                .border_b_1()
                                .border_color(Hsla::from(editor.line))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Exportinstellingen"),
                                )
                                .child(
                                    div()
                                        .id("export-settings-close")
                                        .size(px(28.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(7.))
                                        .cursor_pointer()
                                        .hover(move |style| style.bg(Hsla::from(editor.ctl)))
                                        .child(
                                            svg()
                                                .path("icons/x.svg")
                                                .size(px(14.))
                                                .text_color(Hsla::from(editor.text_2)),
                                        )
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.close_export_settings(cx);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .id("export-settings")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .px(px(16.))
                                .py(px(14.))
                                .gap(px(16.))
                                .child(self.export_destination_notes(ui, link_disabled, cx))
                                .child(self.export_format_field(ui, format_locked, cx))
                                .child(self.export_resolution_field(ui, &resolutions, cx))
                                .child(self.export_fps_field(ui, fps_options, cx))
                                .when(
                                    ui.format == ExportFormatKind::Mp4 && !ui.cursor_only,
                                    |this| this.child(self.export_quality_field(ui, cx)),
                                )
                                .child(div().h(px(1.)).flex_none().bg(Hsla::from(editor.line)))
                                .child(self.export_advanced(ui, cx)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .px(px(16.))
                                .pt(px(10.))
                                .pb(px(12.))
                                .border_t_1()
                                .border_color(Hsla::from(editor.line))
                                .child(
                                    div()
                                        .id("export-settings-defaults")
                                        .text_size(px(12.))
                                        .text_color(Hsla::from(editor.text_2))
                                        .cursor_pointer()
                                        .hover(move |style| {
                                            style.text_color(Hsla::from(editor.text_1))
                                        })
                                        .child("Standaard herstellen")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.restore_export_defaults(window, cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id("export-settings-done")
                                        .h(px(32.))
                                        .px(px(18.))
                                        .flex()
                                        .items_center()
                                        .rounded(px(8.))
                                        .bg(Hsla::from(editor.accent))
                                        .hover(move |style| style.bg(Hsla::from(editor.accent_2)))
                                        .cursor_pointer()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(gpui::white())
                                        .child("Klaar")
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.close_export_settings(cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The export panel in the editor's bottom-right corner: destination and
    /// the action always in view, the settings scrolling between them, and the
    /// running or finished export taking the settings' place.
    pub(crate) fn render_export_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let editor = theme.editor;
        let card = div()
            .id("export-panel")
            .relative()
            .w(px(EXPORT_PANEL_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .border_1()
            .border_color(self.card_line())
            .bg(self.panel_bg())
            .shadow(editor.card_shadow())
            .overflow_hidden()
            .text_color(Hsla::from(editor.text_1));

        let Some(ui) = self.export.as_ref() else {
            return card.child(
                div()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Exporteren"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(editor.text_2))
                            .child("Opname wordt geladen…"),
                    ),
            );
        };

        let link_disabled = has_transparent_background(&self.project) || ui.cursor_only;
        let show_settings = !ui.phase.shows_progress();
        let estimate = match &ui.preview_stats {
            Some(stats) => format!("≈ {:.0} MB", stats.estimated_size_mb.max(1.)),
            None => "Grootte wordt berekend…".to_string(),
        };

        card.child(
            div()
                .flex_none()
                .px(px(12.))
                .pt(px(12.))
                .pb(px(10.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .border_b_1()
                .border_color(Hsla::from(editor.line))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(
                                    svg()
                                        .path("icons/upload-arrow.svg")
                                        .size(px(14.))
                                        .text_color(Hsla::from(editor.text_2)),
                                )
                                .child("Exporteren"),
                        )
                        .when(show_settings, |this| {
                            this.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(Hsla::from(editor.text_2))
                                    .child(estimate),
                            )
                        }),
                )
                .child(
                    div()
                        .when(ui.phase.is_busy(), |this| this.opacity(0.5))
                        .child(self.export_destination_switch(ui, link_disabled, cx)),
                ),
        )
        .when(show_settings, |this| {
            this.child(self.render_export_overview(ui, cx)).child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .pt(px(10.))
                    .pb(px(12.))
                    .border_t_1()
                    .border_color(Hsla::from(editor.line))
                    .child(self.export_cta(ui, cx)),
            )
        })
        .when(!show_settings, |this| {
            this.child(self.render_export_status(ui))
                .child(self.render_export_status_actions(ui, cx))
        })
        .when(ui.phase == ExportPhase::ChoosingFile, |this| {
            this.child(div().absolute().inset_0().occlude())
        })
    }

    /// The Bits per pixel slider's window-wide drag layer: gpui has no pointer
    /// capture, so the drag would end the moment the pointer left the track.
    pub(crate) fn render_export_drag_layer(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        self.export.as_ref().filter(|ui| ui.bpp_dragging)?;
        Some(ui::Slider::drag_layer(
            "export-bpp-drag",
            cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.export_bpp_drag_to(event.position.x, cx);
            }),
            cx.listener(|this, _: &MouseUpEvent, window, cx| {
                this.export_bpp_mouse_up(window, cx);
            }),
        ))
    }

    fn export_heading(&self, ui: &ExportUi) -> &'static str {
        match ui.phase {
            ExportPhase::Starting => "Export voorbereiden",
            ExportPhase::Rendering if ui.cursor_only => "Cursorspoor renderen",
            ExportPhase::Rendering if ui.format == ExportFormatKind::Gif => "GIF renderen",
            ExportPhase::Rendering => "Video renderen",
            ExportPhase::Copying if ui.destination == ExportDestination::Clipboard => {
                "Kopiëren naar klembord"
            }
            ExportPhase::Copying => "Opslaan als bestand",
            ExportPhase::Uploading if crate::direct_drive::enabled() => "Uploaden naar Drive",
            ExportPhase::Uploading if ui.reuploading => "Opnieuw uploaden naar je link",
            ExportPhase::Uploading => "Deelbare link maken",
            ExportPhase::Done if ui.destination == ExportDestination::Clipboard => {
                "Gekopieerd naar klembord"
            }
            ExportPhase::Done if ui.destination == ExportDestination::Link && ui.reuploading => {
                "Opnieuw uploaden voltooid"
            }
            ExportPhase::Done
                if ui.destination == ExportDestination::Link && crate::direct_drive::enabled() =>
            {
                "Opname staat veilig in Drive"
            }
            ExportPhase::Done if ui.destination == ExportDestination::Link => "Upload voltooid",
            ExportPhase::Done => "Opgeslagen als bestand",
            ExportPhase::Failed if ui.clipboard_retry_path().is_some() => {
                "Export klaar, kopiëren mislukt"
            }
            ExportPhase::Failed => "Export mislukt",
            ExportPhase::Idle | ExportPhase::ChoosingFile => "",
        }
    }

    fn render_export_status(&self, ui: &ExportUi) -> impl IntoElement {
        let theme = self.theme;
        let editor = theme.editor;
        let fraction = if ui.phase == ExportPhase::Uploading {
            Some(ui.upload_progress.clamp(0.0, 1.0))
        } else if ui.total_frames == 0 {
            None
        } else {
            Some((ui.rendered as f32 / ui.total_frames as f32).clamp(0.0, 1.0))
        };
        let busy = ui.phase.is_busy();
        let done = ui.phase == ExportPhase::Done;
        let detail = match ui.phase {
            ExportPhase::Rendering if ui.total_frames > 0 => {
                Some(format!("{} van {} frames", ui.rendered, ui.total_frames))
            }
            ExportPhase::Done if ui.destination == ExportDestination::Link => Some(
                if crate::direct_drive::enabled() {
                    "Privé in je Drive. Deel hem via Google Drive als je wilt."
                } else if ui.reuploading {
                    "Je nieuwste versie staat op dezelfde link"
                } else {
                    "Je opname is geüpload"
                }
                .to_string(),
            ),
            ExportPhase::Done => ui
                .output_path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned()),
            _ if busy && crate::direct_drive::enabled() => {
                Some("Je kunt gewoon verder kijken. Annuleren mag altijd.".to_string())
            }
            _ => None,
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .justify_center()
            .gap(px(10.))
            .px(px(16.))
            .when(done, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .size(px(36.))
                                .flex_none()
                                .rounded_full()
                                .bg(Hsla::from(editor.accent))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    svg()
                                        .path("icons/check.svg")
                                        .size(px(18.))
                                        .text_color(gpui::white()),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(self.export_heading(ui)),
                                )
                                .children(detail.clone().map(|detail| {
                                    div()
                                        .text_size(px(12.))
                                        .text_color(Hsla::from(editor.text_2))
                                        .truncate()
                                        .child(detail)
                                })),
                        ),
                )
            })
            .when(!done, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.export_heading(ui)),
                        )
                        .children(fraction.filter(|_| busy).map(|fraction| {
                            div()
                                .text_size(px(12.))
                                .text_color(Hsla::from(editor.text_2))
                                .child(format!("{:.0}%", fraction * 100.))
                        })),
                )
                .when(busy, |this| {
                    this.child(
                        div()
                            .h(px(6.))
                            .w_full()
                            .rounded_full()
                            .bg(Hsla::from(editor.ctl))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(Hsla::from(editor.accent))
                                    .w(gpui::relative(fraction.unwrap_or(0.08).max(0.02))),
                            ),
                    )
                })
                .children(detail.map(|detail| {
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(editor.text_2))
                        .child(detail)
                }))
                .when_some(ui.error.clone(), |this, error| {
                    this.child(
                        div()
                            .id("export-error")
                            .max_h(px(64.))
                            .overflow_y_scroll()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.red_9))
                            .child(error),
                    )
                })
            })
    }

    fn render_export_status_actions(
        &self,
        ui: &ExportUi,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let editor = theme.editor;
        let busy = ui.phase.is_busy();
        let finished = matches!(ui.phase, ExportPhase::Done | ExportPhase::Failed);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            let (bg, hover, text) = if primary {
                (
                    Hsla::from(editor.accent),
                    Hsla::from(editor.accent_2),
                    gpui::white(),
                )
            } else {
                (
                    Hsla::from(editor.ctl),
                    Hsla::from(editor.ctl_hover),
                    Hsla::from(editor.text_1),
                )
            };
            div()
                .id(id)
                .tab_index(0)
                .flex_1()
                .min_w_0()
                .h(px(40.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.))
                .bg(bg)
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(label)
        };

        div()
            .flex_none()
            .flex()
            .flex_row()
            .gap(px(8.))
            .px(px(12.))
            .pt(px(10.))
            .pb(px(12.))
            .border_t_1()
            .border_color(Hsla::from(editor.line))
            .when(busy, |this| {
                this.child(
                    button("export-cancel", "Annuleren", false).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.request_cancel_export(window, cx);
                        },
                    )),
                )
            })
            .when(finished && ui.clipboard_retry_path().is_some(), |this| {
                this.child(
                    button("export-retry-copy", "Opnieuw kopiëren", true).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.retry_clipboard_copy(window, cx);
                        },
                    )),
                )
            })
            .when(
                ui.phase == ExportPhase::Done && ui.destination == ExportDestination::File,
                |this| {
                    let path = ui.output_path.clone();
                    this.child(button("export-open-file", "Bestand openen", true).on_click(
                        move |_, _, _| {
                            if let Some(path) = &path {
                                library::open_path(path);
                            }
                        },
                    ))
                },
            )
            .when(
                ui.phase == ExportPhase::Done
                    && ui.destination == ExportDestination::Link
                    && ui.share_link.is_some(),
                |this| {
                    let open = ui.share_link.clone().unwrap_or_default();
                    this.child(
                        button(
                            "export-copy-link",
                            if ui.copy_link_pressed {
                                "Link gekopieerd"
                            } else {
                                "Link kopiëren"
                            },
                            true,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.copy_share_link(window, cx);
                        })),
                    )
                    .child(
                        button("export-open-link", "Openen", false).on_click(move |_, _, cx| {
                            cx.open_url(&open);
                        }),
                    )
                },
            )
            .when(finished, |this| {
                this.child(
                    button("export-new", "Nieuwe export", false).on_click(cx.listener(
                        |this, _, _window, cx| {
                            this.reset_export(cx);
                        },
                    )),
                )
            })
    }

    fn request_cancel_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        let cancel = ui.cancel.clone();
        #[cfg(target_os = "linux")]
        {
            if !ui.phase.is_busy() {
                return;
            }
            let response = crate::editor_modal::confirm_cancel_export(window, cx);
            cx.spawn_in(window, async move |this, cx| {
                let confirmed = response.await;
                let _ = this.update_in(cx, |this, _, cx| {
                    if let Some(ui) = this.export.as_mut()
                        && ui.phase.is_busy()
                    {
                        cancel_matching_export(&ui.cancel, &cancel, confirmed);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        #[cfg(not(target_os = "linux"))]
        cx.spawn_in(window, async move |this, cx| {
            let confirmed = platform::confirm_dialog(
                "Export annuleren?",
                "Weet je zeker dat je de export wilt annuleren?",
                "Export annuleren",
                "Doorgaan",
                true,
            );
            let _ = this.update(cx, |this, cx| {
                if let Some(ui) = this.export.as_mut() {
                    cancel_matching_export(&ui.cancel, &cancel, confirmed);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export_cta(&self, ui: &ExportUi, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let editor = theme.editor;
        let signed_in = !ui.auth_rejected && store::auth_snapshot().signed_in();
        let sign_in_pending = SignIn::global(cx).read(cx).is_pending();
        let (label, icon, primary) = match ui.destination {
            ExportDestination::File => ("Exporteren naar bestand", Some("icons/folder.svg"), true),
            ExportDestination::Clipboard => {
                ("Kopiëren naar klembord", Some("icons/copy.svg"), true)
            }
            ExportDestination::Link if crate::direct_drive::enabled() => (
                if cap_direct_drive::is_connected() {
                    "Uploaden naar Drive"
                } else {
                    "Koppel Google Drive in Instellingen"
                },
                Some("icons/link.svg"),
                true,
            ),
            ExportDestination::Link if sign_in_pending => ("Inloggen annuleren", None, false),
            ExportDestination::Link if !signed_in => {
                ("Log in om te delen", Some("icons/link.svg"), true)
            }
            ExportDestination::Link => (
                if self.sharing.is_some() {
                    "Opnieuw uploaden naar dezelfde link"
                } else {
                    "Deelbare link maken"
                },
                Some("icons/link.svg"),
                true,
            ),
        };
        let busy = ui.phase.is_busy();
        let (bg, hover_bg, text) = if primary {
            (
                Hsla::from(editor.accent),
                Hsla::from(editor.accent_2),
                gpui::white(),
            )
        } else {
            (
                Hsla::from(editor.ctl),
                Hsla::from(editor.ctl_hover),
                Hsla::from(editor.text_1),
            )
        };

        div()
            .id("export-cta")
            .tab_index(0)
            .w_full()
            .h(px(40.))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .rounded(px(10.))
            .bg(bg)
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(text)
            .when(primary, |this| {
                this.shadow(vec![gpui::BoxShadow {
                    color: gpui::hsla(0., 0., 1., 0.18),
                    offset: gpui::point(px(0.), px(1.)),
                    blur_radius: px(0.),
                    spread_radius: px(0.),
                    inset: true,
                }])
            })
            .when(busy, |this| this.opacity(0.5))
            .when(!busy, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(hover_bg))
                    .on_click(cx.listener(|this, _, window, cx| this.start_export(window, cx)))
            })
            .children(icon.map(|icon| svg().path(icon).size(px(16.)).text_color(text)))
            .child(label)
    }

    fn export_section(&self, name: &'static str, icon: &'static str) -> ui::Field {
        ui::Field::section(&self.theme, name)
            .icon(icon)
            .icon_size(px(14.))
            .gap(px(8.))
    }

    fn export_destination_switch(
        &self,
        ui: &ExportUi,
        link_disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        div().w_full().child(
            ui::SegmentedControl::editor(
                &theme,
                "export-destination",
                ExportDestination::ALL
                    .iter()
                    .map(|dest| {
                        let label =
                            if *dest == ExportDestination::Link && crate::direct_drive::enabled() {
                                "Drive"
                            } else if *dest == ExportDestination::Link && self.sharing.is_some() {
                                "Opnieuw uploaden"
                            } else {
                                dest.label()
                            };
                        let mut option = ui::SegmentOption::new(label, ui.destination == *dest)
                            .disabled(*dest == ExportDestination::Link && link_disabled);
                        option.icon = Some(dest.icon().into());
                        option
                    })
                    .collect(),
            )
            .stretch()
            .item_height(px(30.))
            .on_select(cx.listener(|this, index: &usize, window, cx| {
                let Some(dest) = ui::option_at(ExportDestination::ALL, *index) else {
                    return;
                };
                if let Some(ui) = this.export.as_mut() {
                    if ui.phase.is_busy() {
                        return;
                    }
                    ui.phase = ExportPhase::Idle;
                    ui.error = None;
                    ui.copy_link_pressed = false;
                    if dest == ExportDestination::Link
                        && (has_transparent_background(&this.project) || ui.cursor_only)
                    {
                        return;
                    }
                    ui.destination = dest;
                    if dest == ExportDestination::Link && ui.format == ExportFormatKind::Gif {
                        ui.format = ExportFormatKind::Mp4;
                    }
                    ui.persist();
                }
                this.refresh_export_preview(window, cx);
                cx.notify();
            })),
        )
    }

    fn export_destination_notes(
        &self,
        ui: &ExportUi,
        link_disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let editor = self.theme.editor;
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(link_disabled, |field| {
                field.child(
                    div()
                        .text_size(px(11.))
                        .text_color(Hsla::from(editor.text_3))
                        .child(if ui.cursor_only {
                            "Een export van alleen de cursor kan alleen naar een bestand of het klembord."
                        } else {
                            "Een transparante export kan alleen naar een bestand of het klembord."
                        }),
                )
            })
            .when(
                ui.destination == ExportDestination::Link && self.sharing.is_some() && !crate::direct_drive::enabled(),
                |field| {
                    let link = self.sharing.as_ref()
                        .map(|sharing| sharing.link.clone())
                        .unwrap_or_default();
                    field.child(
                        div()
                            .p(px(12.))
                            .rounded(px(10.))
                            .bg(Hsla::from(editor.card_2))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(Hsla::from(editor.text_1))
                                    .child("Update your existing link"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .line_height(px(15.))
                                    .text_color(Hsla::from(editor.text_3))
                                    .child("Reupload replaces the video at this link with your latest edit. Everyone with the link will see the updated version."),
                            )
                            .child(
                                div()
                                    .id("reupload-existing-link")
                                    .text_size(px(11.5))
                                    .text_color(Hsla::from(editor.accent))
                                    .truncate()
                                    .cursor_pointer()
                                    .child(link.clone())
                                    .on_click(move |_, _, cx| cx.open_url(&link)),
                            ),
                    )
                },
            )
            .when(
                ui.destination == ExportDestination::Link
                    && !crate::direct_drive::enabled()
                    && self.sharing.is_none()
                    && store::auth_snapshot().organizations.len() > 1,
                |this| {
                    let orgs = store::auth_snapshot().organizations;
                    let selected = ui
                        .organization_id
                        .clone()
                        .or_else(|| orgs.first().map(|org| org.id.clone()));
                    let label = orgs
                        .iter()
                        .find(|org| Some(org.id.as_str()) == selected.as_deref())
                        .or(orgs.first())
                        .map(|org| org.name.clone())
                        .unwrap_or_else(|| "Organization".into());
                    this.child(
                        div()
                            .id("export-organization")
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .h(px(30.))
                            .px(px(10.))
                            .rounded(px(7.))
                            .bg(Hsla::from(editor.ctl))
                            .cursor_pointer()
                            .hover(move |style| style.bg(Hsla::from(editor.ctl_hover)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let orgs = store::auth_snapshot().organizations;
                                if orgs.is_empty() {
                                    return;
                                }
                                if let Some(ui) = this.export.as_mut() {
                                    let current = ui
                                        .organization_id
                                        .as_deref()
                                        .or(orgs.first().map(|org| org.id.as_str()));
                                    let index = orgs
                                        .iter()
                                        .position(|org| Some(org.id.as_str()) == current)
                                        .unwrap_or(0);
                                    let next = orgs[(index + 1) % orgs.len()].id.clone();
                                    ui.organization_id = Some(next);
                                    ui.persist();
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(Hsla::from(editor.text_2))
                                    .child("Organization"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(4.))
                                    .text_size(px(12.))
                                    .text_color(Hsla::from(editor.text_1))
                                    .child(label)
                                    .child(
                                        svg()
                                            .path("icons/caret-down.svg")
                                            .size(px(14.))
                                            .text_color(Hsla::from(editor.text_3)),
                                    ),
                            ),
                    )
                },
            )
    }

    fn export_format_field(
        &self,
        ui: &ExportUi,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let options = [ExportFormatKind::Mp4, ExportFormatKind::Gif];
        self.export_section("Formaat", "icons/video.svg")
            .disabled(locked)
            .child(
                ui::SegmentedControl::editor(
                    &theme,
                    "export-format",
                    options
                        .iter()
                        .map(|format| {
                            ui::SegmentOption::new(format.label(), ui.format == *format)
                                .disabled(locked)
                        })
                        .collect(),
                )
                .stretch()
                .item_height(px(SEGMENT_HEIGHT))
                .on_select(cx.listener(move |this, index: &usize, window, cx| {
                    if locked {
                        return;
                    }
                    let Some(format) = options.get(*index).copied() else {
                        return;
                    };
                    if let Some(ui) = this.export.as_mut() {
                        ui.format = format;
                        if format == ExportFormatKind::Gif {
                            if ui.destination == ExportDestination::Link {
                                ui.destination = ExportDestination::File;
                            }
                            if ui.resolution == ExportResolution::P4k {
                                ui.resolution = ExportResolution::P1080;
                            }
                        }
                    }
                    this.normalize_loaded_export_fps();
                    if let Some(ui) = this.export.as_mut() {
                        ui.persist();
                    }
                    this.refresh_export_preview(window, cx);
                    cx.notify();
                })),
            )
    }

    fn export_resolution_field(
        &self,
        ui: &ExportUi,
        resolutions: &[ExportResolution],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let resolutions = resolutions.to_vec();
        self.export_section("Resolutie", "icons/monitor-outline.svg")
            .child(
                ui::SegmentedControl::editor(
                    &theme,
                    "export-resolution",
                    resolutions
                        .iter()
                        .map(|res| ui::SegmentOption::new(res.label(), ui.resolution == *res))
                        .collect(),
                )
                .stretch()
                .item_height(px(SEGMENT_HEIGHT))
                .on_select(cx.listener(move |this, index: &usize, window, cx| {
                    let Some(resolution) = resolutions.get(*index).copied() else {
                        return;
                    };
                    if let Some(ui) = this.export.as_mut() {
                        ui.resolution = resolution;
                        ui.persist();
                    }
                    this.refresh_export_preview(window, cx);
                    cx.notify();
                })),
            )
    }

    fn export_fps_field(
        &self,
        ui: &ExportUi,
        options: &[u32],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let options = options.to_vec();
        self.export_section("Framerate", "icons/gauge.svg").child(
            ui::SegmentedControl::editor(
                &theme,
                "export-fps",
                options
                    .iter()
                    .map(|fps| ui::SegmentOption::new(format!("{fps} FPS"), ui.fps == *fps))
                    .collect(),
            )
            .stretch()
            .item_height(px(SEGMENT_HEIGHT))
            .on_select(cx.listener(move |this, index: &usize, window, cx| {
                let Some(fps) = options.get(*index).copied() else {
                    return;
                };
                if let Some(ui) = this.export.as_mut() {
                    ui.fps = fps;
                    ui.persist();
                }
                this.refresh_export_preview(window, cx);
                cx.notify();
            })),
        )
    }

    fn export_quality_field(&self, ui: &ExportUi, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let editor = theme.editor;
        self.export_section("Kwaliteit", "icons/gem.svg")
            .child(
                ui::SegmentedControl::editor(
                    &theme,
                    "export-quality",
                    COMPRESSION_PRESETS
                        .iter()
                        .map(|(value, label)| {
                            ui::SegmentOption::new(
                                *label,
                                !ui.is_custom_bpp() && matches_compression(ui.compression, *value),
                            )
                        })
                        .collect(),
                )
                .stretch()
                .item_height(px(SEGMENT_HEIGHT))
                .on_select(cx.listener(move |this, index: &usize, window, cx| {
                    let Some((compression, _)) = COMPRESSION_PRESETS.get(*index).copied() else {
                        return;
                    };
                    if let Some(ui) = this.export.as_mut() {
                        ui.compression = compression;
                        ui.custom_bpp = None;
                        ui.persist();
                    }
                    this.refresh_export_preview(window, cx);
                    cx.notify();
                })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .px(px(2.))
                    .text_size(px(10.5))
                    .text_color(Hsla::from(editor.text_3))
                    .child("Kleiner bestand")
                    .child("Groter bestand"),
            )
            .child(self.export_optimize_row(ui, cx))
    }

    fn export_toggle_row(
        &self,
        id: &'static str,
        title: &'static str,
        description: &'static str,
        checked: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> impl IntoElement {
        let editor = self.theme.editor;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .min_h(px(34.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(Hsla::from(editor.text_1))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(14.))
                            .text_color(Hsla::from(editor.text_3))
                            .child(description),
                    ),
            )
            .child(ui::Toggle::plain(&self.theme, id, checked).on_click(on_click))
    }

    fn export_optimize_row(&self, ui: &ExportUi, cx: &mut Context<Self>) -> impl IntoElement {
        self.export_toggle_row(
            "export-optimize",
            "Bestandsgrootte optimaliseren",
            "Re-encodes with software for much smaller files (slower)",
            ui.optimize_filesize,
            cx.listener(|this, _, window, cx| {
                if let Some(ui) = this.export.as_mut() {
                    ui.optimize_filesize = !ui.optimize_filesize;
                    ui.persist();
                }
                this.refresh_export_preview(window, cx);
                cx.notify();
            }),
        )
    }

    fn export_advanced(&self, ui: &ExportUi, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let editor = theme.editor;
        let show_bpp = ui.format == ExportFormatKind::Mp4 && !ui.cursor_only;
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .id("export-advanced-toggle")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .h(px(30.))
                    .mx(px(-6.))
                    .px(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(Hsla::from(editor.text_2))
                    .hover(move |style| {
                        style
                            .bg(Hsla::from(editor.ctl))
                            .text_color(Hsla::from(editor.text_1))
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        if let Some(ui) = this.export.as_mut() {
                            ui.advanced_open = !ui.advanced_open;
                            ui.persist();
                        }
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path("icons/sliders-horizontal.svg")
                            .size(px(14.))
                            .text_color(Hsla::from(editor.text_2)),
                    )
                    .child("Geavanceerd")
                    .child(
                        svg()
                            .path(if ui.advanced_open {
                                "icons/chevron-up.svg"
                            } else {
                                "icons/chevron-down.svg"
                            })
                            .size(px(14.))
                            .ml_auto()
                            .text_color(Hsla::from(editor.text_3)),
                    ),
            )
            .when(ui.advanced_open, |this| {
                this.child(self.export_toggle_row(
                    "export-cursor-only",
                    "Alleen cursor exporteren",
                    "Keeps the same cursor motion and clicks on a transparent background",
                    ui.cursor_only,
                    cx.listener(|this, _, window, cx| {
                        if let Some(ui) = this.export.as_mut() {
                            ui.cursor_only = !ui.cursor_only;
                            if ui.cursor_only && ui.destination == ExportDestination::Link {
                                ui.destination = ExportDestination::File;
                            }
                            ui.persist();
                        }
                        this.refresh_export_preview(window, cx);
                        cx.notify();
                    }),
                ))
                .when(ui.cursor_only, |this| {
                    this.child(
                        div()
                            .mt(px(4.))
                            .p(px(12.))
                            .rounded(px(10.))
                            .bg(Hsla::from(editor.card_2))
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(px(8.))
                            .child(
                                svg()
                                    .path("icons/triangle-alert.svg")
                                    .size(px(14.))
                                    .flex_shrink_0()
                                    .mt(px(1.))
                                    .text_color(Hsla::from(editor.text_2)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .line_height(px(15.))
                                    .text_color(Hsla::from(editor.text_2))
                                    .child(
                                        "Exports as a transparent MOV. Files are large and best for compositing or editing.",
                                    ),
                            ),
                    )
                })
                .when(show_bpp, |this| {
                    let fraction = ((ui.bpp() - BPP_MIN) / (BPP_MAX - BPP_MIN)).clamp(0., 1.);
                    this.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(34.))
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_none()
                                    .min_w(px(96.))
                                    .text_size(px(13.))
                                    .text_color(Hsla::from(editor.text_1))
                                    .child("Bits per pixel"),
                            )
                            .child(
                                div()
                                    .id("export-bpp-row")
                                    .flex_1()
                                    .min_w_0()
                                    .px(px(4.))
                                    .h(px(32.))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .child(
                                        ui::Slider::new(
                                            "export-bpp",
                                            fraction,
                                            ui.bpp_track.clone(),
                                        )
                                        .flex()
                                        .row_height(px(28.))
                                        .track(px(3.), Hsla::from(editor.ctl_active))
                                        .fill(Hsla::from(editor.accent))
                                        .thumb(
                                            px(14.),
                                            Hsla::from(editor.thumb),
                                            Some(gpui::hsla(0., 0., 0., 0.12)),
                                        )
                                        .thumb_shadow()
                                        .on_drag_start(cx.listener(
                                            |this, event: &MouseDownEvent, _window, cx| {
                                                this.export_bpp_mouse_down(event, cx);
                                            },
                                        )),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .min_w(px(36.))
                                    .text_right()
                                    .text_size(px(11.))
                                    .text_color(Hsla::from(editor.text_3))
                                    .child(format!("{:.2}", ui.bpp())),
                            ),
                    )
                    .when(ui.is_custom_bpp(), |this| {
                        this.child(
                            div()
                                .text_size(px(11.))
                                .text_color(Hsla::from(editor.text_3))
                                .child("Eigen bitrate in gebruik"),
                        )
                    })
                })
                .when(cfg!(target_os = "macos") && show_bpp, |this| {
                    this.child(self.export_toggle_row(
                        "export-force-ffmpeg",
                        "FFmpeg-decoder forceren",
                        "Skip hardware decoder (auto-fallback enabled)",
                        ui.force_ffmpeg,
                        cx.listener(|this, _, window, cx| {
                            if let Some(ui) = this.export.as_mut() {
                                ui.force_ffmpeg = !ui.force_ffmpeg;
                                ui.persist();
                            }
                            this.refresh_export_preview(window, cx);
                            cx.notify();
                        }),
                    ))
                })
            })
    }

    fn export_bpp_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if let Some(ui) = self.export.as_mut() {
            ui.bpp_dragging = true;
        }
        self.export_bpp_drag_to(event.position.x, cx);
        cx.notify();
    }

    fn export_bpp_drag_to(&mut self, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        let Some(bounds) = ui.bpp_track.get() else {
            return;
        };
        let Some(fraction) = ui::fraction_from_x(x, bounds) else {
            return;
        };
        let value = ui::snap_to_step(
            ui::value_from_fraction(fraction, BPP_MIN, BPP_MAX),
            BPP_MIN,
            BPP_MAX,
            BPP_STEP,
        );
        if (value - ui.bpp()).abs() < f32::EPSILON {
            return;
        }
        ui.custom_bpp = Some(value);
        if let Some((preset, _)) = COMPRESSION_PRESETS
            .iter()
            .find(|(preset, _)| (preset.bits_per_pixel() - value).abs() < 0.001)
        {
            ui.compression = *preset;
        }
        cx.notify();
    }

    fn export_bpp_mouse_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = self.export.as_mut() else {
            return;
        };
        if !ui.bpp_dragging {
            return;
        }
        ui.bpp_dragging = false;
        ui.persist();
        self.refresh_export_preview(window, cx);
        cx.notify();
    }
}

async fn confirm_export_resources(
    this: &gpui::WeakEntity<EditorWindow>,
    cx: &mut gpui::AsyncWindowContext,
    resources: &ExportResources,
    cancel: &Arc<AtomicBool>,
) -> bool {
    let check = {
        let resources = resources.clone();
        gpui_tokio::Tokio::spawn(cx, async move { resources.check().await }).await
    };
    let result = async {
        let warning = check.map_err(|error| error.to_string())??;
        if let Some(message) = warning {
            #[cfg(target_os = "linux")]
            let confirmed = {
                let response = loop {
                    let pending = this.update_in(cx, |this, window, cx| {
                        let current = this.export.as_ref().is_some_and(|ui| {
                            Arc::ptr_eq(&ui.cancel, cancel)
                                && !ui.close_requested
                                && !cancel.load(Ordering::Acquire)
                        });
                        let response = (current && !window.has_active_prompt()).then(|| {
                            crate::editor_modal::confirm_action(
                                "Check export resources",
                                &message,
                                "Proceed anyway",
                                "Go back",
                                window,
                                cx,
                            )
                        });
                        (current, response)
                    });
                    match pending {
                        Ok((true, Some(response))) => break response,
                        Ok((true, None)) => {
                            cx.background_executor()
                                .timer(Duration::from_millis(50))
                                .await;
                        }
                        _ => return Ok(false),
                    }
                };
                response.await
            };
            #[cfg(not(target_os = "linux"))]
            let confirmed = {
                let current = this
                    .update(cx, |this, _| {
                        this.export.as_ref().is_some_and(|ui| {
                            Arc::ptr_eq(&ui.cancel, cancel)
                                && !ui.close_requested
                                && !cancel.load(Ordering::Acquire)
                        })
                    })
                    .unwrap_or(false);
                if !current {
                    return Ok(false);
                }
                platform::confirm_dialog(
                    "Check export resources",
                    &message,
                    "Proceed anyway",
                    "Go back",
                    false,
                )
            };
            if !confirmed {
                return Ok(false);
            }
            let resources = resources.clone();
            let _ = gpui_tokio::Tokio::spawn(cx, async move { resources.check().await })
                .await
                .map_err(|error| error.to_string())??;
        }
        Ok::<bool, String>(true)
    }
    .await;
    this.update(cx, |this, cx| {
        let Some(ui) = this.export.as_mut() else {
            return false;
        };
        if !Arc::ptr_eq(&ui.cancel, cancel) {
            return false;
        }
        let accepted = result.as_ref().is_ok_and(|accepted| *accepted)
            && !ui.close_requested
            && !cancel.load(Ordering::Acquire);
        if !accepted {
            match result {
                Err(error) if !ui.close_requested && !cancel.load(Ordering::Acquire) => {
                    ui.phase = ExportPhase::Failed;
                    ui.error = Some(error);
                }
                _ => ui.phase = ExportPhase::Idle,
            }
            cx.notify();
        }
        accepted
    })
    .unwrap_or(false)
}

fn cancel_matching_export(current: &Arc<AtomicBool>, requested: &Arc<AtomicBool>, confirmed: bool) {
    if confirmed && Arc::ptr_eq(current, requested) {
        current.store(true, Ordering::Relaxed);
    }
}

fn show_upload_failure(
    this: &gpui::WeakEntity<EditorWindow>,
    cx: &mut gpui::AsyncWindowContext,
    message: &str,
    dialog: bool,
) {
    let message = message.to_string();
    let _ = this.update(cx, |this, cx| {
        if let Some(ui) = this.export.as_mut() {
            ui.phase = ExportPhase::Failed;
            ui.error = Some(message.clone());
        }
        cx.notify();
    });
    if dialog {
        platform::alert_dialog("Failed to upload recording", &message);
    }
}

fn matches_compression(current: ExportCompression, expected: ExportCompression) -> bool {
    std::mem::discriminant(&current) == std::mem::discriminant(&expected)
}

#[allow(clippy::too_many_arguments)]
async fn run_export(
    project_path: PathBuf,
    project: cap_project::ProjectConfiguration,
    format: ExportFormatKind,
    cursor_only: bool,
    fps: u32,
    width: u32,
    height: u32,
    compression: ExportCompression,
    custom_bpp: Option<f32>,
    optimize: bool,
    force: bool,
    save_path: Option<PathBuf>,
    progress_tx: flume::Sender<(u32, u32)>,
    cancel: Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Export cancelled".into());
    }
    let mut builder = ExporterBase::builder(project_path).with_force_ffmpeg_decoder(force);
    if cursor_only {
        builder = builder.with_config(make_cursor_only_project(project));
    } else {
        builder = builder.with_config(project);
    }
    if let Some(path) = save_path.clone() {
        builder = builder.with_output_path(path);
    }

    enum PreparedBase {
        Mp4(cap_export::Mp4ExporterBase),
        Other(ExporterBase),
    }
    let (base, total) = if !cursor_only && format != ExportFormatKind::Gif {
        let base = builder
            .build_for_mp4(cancel.clone())
            .await
            .map_err(|error| error.to_string())?;
        let total = base.total_frames(fps);
        (PreparedBase::Mp4(base), total)
    } else {
        let base = builder
            .build_video_only()
            .await
            .map_err(|error| error.to_string())?;
        let total = base.total_frames(fps);
        (PreparedBase::Other(base), total)
    };
    let _ = progress_tx.send((0, total));

    let progress = {
        let progress_tx = progress_tx.clone();
        let cancel = cancel.clone();
        move |frame_index: u32| {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            progress_tx
                .send(((frame_index + 1).min(total), total))
                .is_ok()
        }
    };

    let resolution = XY::new(width, height);
    match base {
        PreparedBase::Other(base) if cursor_only => {
            MovExportSettings {
                fps,
                resolution_base: resolution,
                cursor_only: true,
            }
            .export(base, progress)
            .await
        }
        PreparedBase::Other(base) => {
            GifExportSettings {
                fps,
                resolution_base: resolution,
                quality: None,
            }
            .export(base, progress)
            .await
        }
        PreparedBase::Mp4(base) => {
            Mp4ExportSettings {
                fps,
                resolution_base: resolution,
                compression,
                custom_bpp,
                force_ffmpeg_decoder: force,
                optimize_filesize: optimize,
            }
            .export_prepared(base, progress)
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExportDestination, ExportPhase, ExportUi, cancel_matching_export, export_file_name,
    };
    use crate::store::ExportPrefs;
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn clipboard_export() -> ExportUi {
        ExportUi::from_preferences(ExportPrefs {
            format: "Mp4".into(),
            fps: 30,
            export_to: "clipboard".into(),
            resolution: "720p".into(),
            compression: "Maximum".into(),
            optimize_filesize: false,
            cursor_only: false,
            custom_bpp: None,
            force_ffmpeg_decoder: false,
            advanced_open: false,
            organization_id: None,
            drive_folder_id: None,
            drive_folder_path: None,
        })
    }

    #[test]
    fn export_names_sanitize_titles_and_preserve_the_format_extension() {
        for extension in ["mp4", "gif", "mov"] {
            for (title, expected) in [
                ("Sprint 1/2", "Sprint 12"),
                (r#"a/b\c:d*e?f"g<h>i|j"#, "abcdefghij"),
                ("\0\n", "Kiekes Recording"),
                ("/\\:*?\"<>|", "Kiekes Recording"),
                ("...", "Kiekes Recording"),
                ("   ", "Kiekes Recording"),
                ("CON", "Kiekes Recording"),
                ("Clip final. ", "Clip final"),
                ("My Clip 2026", "My Clip 2026"),
                ("你好 🎬", "你好 🎬"),
            ] {
                let filename = export_file_name(title, extension);
                assert_eq!(filename, format!("{expected}.{extension}"), "{title:?}");
                assert_eq!(PathBuf::from(&filename).components().count(), 1);
            }
        }
    }

    #[test]
    fn long_export_names_leave_room_for_the_format_extension() {
        for title in ["a".repeat(300), "🎬".repeat(100)] {
            for extension in ["mp4", "gif", "mov"] {
                let filename = export_file_name(&title, extension);
                assert!(filename.len() <= 255, "{} bytes", filename.len());
                assert!(filename.ends_with(&format!(".{extension}")));
                assert!(
                    title.starts_with(filename.strip_suffix(&format!(".{extension}")).unwrap())
                );
            }
        }
    }

    #[test]
    fn stale_preview_results_cannot_replace_the_latest_or_reopened_preview() {
        let mut ui = clipboard_export();
        let previous = ui.preview_request.clone();
        let current = Arc::new(());
        ui.preview_request = current.clone();
        assert!(ui.update_preview(&current, |ui| ui.preview_error = Some("Latest".into())));
        assert!(!ui.update_preview(&previous, |ui| ui.preview_error = Some("Stale".into())));
        assert_eq!(ui.preview_error.as_deref(), Some("Latest"));

        let mut reopened = clipboard_export();
        assert!(!reopened.update_preview(&current, |ui| ui.preview_error = Some("Closed".into())));
        assert!(reopened.preview_error.is_none());
    }

    #[test]
    fn failed_clipboard_copy_retries_the_completed_file() {
        let mut ui = clipboard_export();
        let output = PathBuf::from("finished recording.mp4");
        ui.copy_completed_export(output.clone(), |path| {
            assert_eq!(path, output);
            Err("Clipboard is busy".into())
        });
        assert_eq!(ui.phase, ExportPhase::Failed);
        assert_eq!(ui.error.as_deref(), Some("Clipboard is busy"));
        let retry = ui.clipboard_retry_path().unwrap().to_path_buf();
        assert_eq!(retry, output);

        ui.copy_completed_export(retry, |path| {
            assert_eq!(path, output);
            Ok(())
        });
        assert_eq!(ui.phase, ExportPhase::Done);
        assert_eq!(ui.output_path, Some(output));
        assert!(ui.error.is_none());
        assert!(ui.clipboard_retry_path().is_none());
    }

    #[test]
    fn cancellation_or_close_prevents_late_clipboard_mutation() {
        for closing in [false, true] {
            let mut ui = clipboard_export();
            ui.close_requested = closing;
            ui.cancel.store(!closing, Ordering::Relaxed);
            let output = PathBuf::from("finished recording.mp4");
            ui.copy_completed_export(output.clone(), |_| {
                panic!("cancelled exports must not replace the clipboard")
            });
            assert_eq!(ui.phase, ExportPhase::Idle);
            assert_eq!(ui.output_path, Some(output));
            assert!(ui.clipboard_retry_path().is_none());
        }
    }

    #[test]
    fn completed_share_export_copies_the_link_and_shows_success() {
        let mut ui = clipboard_export();
        ui.destination = ExportDestination::Link;
        ui.phase = ExportPhase::Uploading;
        ui.upload_progress = 0.9;
        let link = "https://cap.link/finished".to_string();
        let mut copied = None;

        ui.complete_share_export(link.clone(), |link| copied = Some(link));

        assert_eq!(ui.phase, ExportPhase::Done);
        assert_eq!(ui.share_link, Some(link.clone()));
        assert_eq!(ui.upload_progress, 1.0);
        assert_eq!(copied, Some(link));
    }

    #[test]
    fn cancellation_or_close_ignores_a_completed_share_export() {
        for closing in [false, true] {
            let mut ui = clipboard_export();
            ui.destination = ExportDestination::Link;
            ui.phase = ExportPhase::Uploading;
            ui.upload_progress = 0.9;
            let completed_link = "https://cap.link/finished".to_string();
            ui.close_requested = closing;
            ui.cancel.store(!closing, Ordering::Relaxed);

            ui.complete_share_export(completed_link, |_| {
                panic!("cancelled share exports must not copy a link or notify")
            });

            assert_eq!(ui.phase, ExportPhase::Idle);
            assert!(ui.share_link.is_none());
            assert_eq!(ui.upload_progress, 0.9);
        }
    }

    #[test]
    fn incomplete_or_non_clipboard_exports_cannot_retry_copy() {
        let mut ui = clipboard_export();
        ui.phase = ExportPhase::Failed;
        assert!(ui.clipboard_retry_path().is_none());
        ui.output_path = Some(PathBuf::from("finished recording.mp4"));
        ui.destination = ExportDestination::File;
        assert!(ui.clipboard_retry_path().is_none());
        ui.destination = ExportDestination::Link;
        assert!(ui.clipboard_retry_path().is_none());
    }

    #[test]
    fn declining_cancel_preserves_the_running_export() {
        let cancel = Arc::new(AtomicBool::new(false));
        cancel_matching_export(&cancel, &cancel, false);
        assert!(!cancel.load(Ordering::Relaxed));
        cancel_matching_export(&cancel, &cancel, true);
        assert!(cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn late_confirmation_cannot_cancel_a_subsequent_export() {
        let previous = Arc::new(AtomicBool::new(false));
        let current = Arc::new(AtomicBool::new(false));
        cancel_matching_export(&current, &previous, true);
        assert!(!current.load(Ordering::Relaxed));
        assert!(!previous.load(Ordering::Relaxed));
    }

    #[test]
    fn choosing_a_file_blocks_duplicate_exports_without_showing_progress() {
        assert!(ExportPhase::ChoosingFile.is_busy());
        assert!(!ExportPhase::ChoosingFile.shows_progress());
    }

    #[test]
    fn cancelling_file_selection_returns_to_the_idle_page() {
        assert!(!ExportPhase::Idle.is_busy());
        assert!(!ExportPhase::Idle.shows_progress());
    }

    #[test]
    fn export_work_and_results_remain_visible() {
        for phase in [
            ExportPhase::Starting,
            ExportPhase::Rendering,
            ExportPhase::Copying,
            ExportPhase::Uploading,
            ExportPhase::Done,
            ExportPhase::Failed,
        ] {
            assert!(phase.shows_progress());
        }
    }
}
