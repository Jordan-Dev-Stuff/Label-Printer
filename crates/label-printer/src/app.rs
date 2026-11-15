//! The main window. Owns the state, draws it, and talks to three background
//! workers: discovery ([`crate::devices`]), rendering and wired printing
//! ([`crate::jobs`]), and Bluetooth LE ([`crate::ble`]).

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, ComboBox, CornerRadius, DragValue, FontId, Frame, Key, Layout,
    Margin, RichText, ScrollArea, Sense, Slider, Stroke, StrokeKind, Ui, Vec2,
};
use tspl_core::discover::{best_printer, DiscoveredPrinter, Discovery, PrinterKind};
use tspl_core::{calibrate_job, feed_job, Config, DEFAULT_DPI};

use crate::ble::{self, Ble, BleCommand, BleEvent, DeviceInfo};
use crate::content::{Halftone, LabelPx, Rotation};
use crate::devices::{self, Watcher};
use crate::filter;
use crate::jobs::{
    self, Destination, JobEvent, JobRequest, Jobs, Layout as JobLayout, Pages, Source,
};
use crate::settings::{self, RememberedBle, Settings, PRESETS};
use crate::theme::{self, icon, ACCENT, BAD, BG, BORDER, GOOD, MUTED, TEXT, WARN};

/// The preview is a full-size black-and-white raster shown much smaller.
/// Mipmaps average dithered areas into grey (as the label looks at arm's
/// length) instead of the moire plain linear filtering produces.
const PREVIEW_TEXTURE: egui::TextureOptions = egui::TextureOptions {
    magnification: egui::TextureFilter::Nearest,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(egui::TextureFilter::Linear),
};

/// Lines kept in the activity log.
const LOG_LIMIT: usize = 300;

/// The printer the Print button sends to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selected {
    /// A discovered USB device, queue or serial port.
    Wired {
        spec: String,
        name: String,
        kind: PrinterKind,
    },
    /// The Bluetooth LE session in the worker.
    Bluetooth { id: String, name: String },
}

/// Bluetooth LE state mirrored from the worker.
#[derive(Default)]
struct BleState {
    scanning: bool,
    connecting: Option<String>,
    connected: Option<(String, String)>,
    devices: Vec<DeviceInfo>,
    filter: String,
    show_all: bool,
    progress: Option<(usize, usize)>,
    /// When the current Bluetooth send started, for the time estimate.
    started: Option<Instant>,
    last_auto_attempt: Option<Instant>,
    /// Automatic reconnect attempts since the last connection.
    auto_attempts: u32,
}

pub struct ThermalApp {
    ctx: egui::Context,
    settings: Settings,
    source: Source,
    page: usize,
    page_count: Option<usize>,
    print_all_pages: bool,

    discovery: Option<Discovery>,
    show_all_printers: bool,
    selected: Option<Selected>,
    ble_state: BleState,

    jobs: Jobs,
    ble: Ble,
    watcher: Watcher,

    preview: Option<egui::TextureHandle>,
    preview_error: Option<String>,
    preview_dirty: bool,
    generation: u64,
    rendering: bool,

    /// Something is printing; the Print button waits.
    busy: Option<String>,
    status: Option<Result<String, String>>,
    log: Vec<String>,
    show_log: bool,
    /// `LABEL_PRINTER_SCREENSHOT`: save the window here once ready, then quit
    /// (used to produce docs/app-preview.png).
    screenshot: Option<(PathBuf, u32)>,
    /// Write settings at the next frame (a printer was remembered or forgotten).
    save_now: bool,
    logo: Option<egui::TextureHandle>,
}

impl ThermalApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_file: Option<PathBuf>,
        storage: Option<settings::StorageLocation>,
    ) -> Self {
        theme::setup(&cc.egui_ctx);
        let settings = cc
            .storage
            .and_then(|storage| {
                eframe::get_value::<Settings>(storage, settings::STORAGE_KEY)
                    .or_else(|| eframe::get_value(storage, settings::LEGACY_STORAGE_KEY))
            })
            .unwrap_or_default()
            .sanitized();

        let wake = |ctx: egui::Context| move || ctx.request_repaint();
        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            settings,
            source: Source::Empty,
            page: 0,
            page_count: None,
            print_all_pages: false,
            discovery: None,
            show_all_printers: false,
            selected: None,
            ble_state: BleState::default(),
            jobs: jobs::spawn(wake(cc.egui_ctx.clone())),
            ble: ble::spawn(wake(cc.egui_ctx.clone())),
            watcher: devices::spawn(wake(cc.egui_ctx.clone())),
            preview: None,
            preview_error: None,
            preview_dirty: true,
            generation: 0,
            rendering: false,
            busy: None,
            status: None,
            log: Vec::new(),
            show_log: false,
            save_now: false,
            screenshot: std::env::var_os("LABEL_PRINTER_SCREENSHOT")
                .map(|path| (PathBuf::from(path), 0)),
            logo: None,
        };
        match &storage {
            Some(location) if location.preferred => {
                app.log(format!("settings: {}", location.file.display()));
            }
            Some(location) => {
                app.log(format!(
                    "settings: {} (the usual AppData folder is not writable)",
                    location.file.display()
                ));
            }
            None => app.fail("Settings cannot be saved on this computer: no writable folder"),
        }
        if let Some(remembered) = &app.settings.ble_printer {
            app.log(format!("remembered Bluetooth printer: {}", remembered.name));
        }
        if let Some(path) = initial_file {
            app.open_file(path);
        }
        app
    }

    // ---------------------------------------------------------------- state

    fn log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > LOG_LIMIT {
            let excess = self.log.len() - LOG_LIMIT;
            self.log.drain(..excess);
        }
    }

    fn ok(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log(message.clone());
        self.status = Some(Ok(message));
    }

    fn fail(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.log(format!("error: {message}"));
        self.status = Some(Err(message));
    }

    fn label_px(&self) -> LabelPx {
        LabelPx {
            width: settings::mm_to_dots(self.settings.width_mm, DEFAULT_DPI),
            height: settings::mm_to_dots(self.settings.height_mm, DEFAULT_DPI),
        }
    }

    fn layout(&self) -> JobLayout {
        JobLayout {
            label: self.label_px(),
            options: self.settings.options,
        }
    }

    fn config(&self) -> Config {
        Config {
            width_mm: f64::from(self.settings.width_mm),
            height_mm: f64::from(self.settings.height_mm),
            gap_mm: f64::from(self.settings.gap_mm),
            dpi: DEFAULT_DPI,
            density: self.settings.density,
            speed: self.settings.speed,
            copies: self.settings.copies,
            // Only inked bands are sent: same label, much less to transfer.
            skip_blank: true,
        }
    }

    fn open_file(&mut self, path: PathBuf) {
        if !jobs::is_supported(&path) {
            self.fail(format!(
                "{} is not an image or PDF",
                jobs::display_name(&path)
            ));
            return;
        }
        self.log(format!("opened {}", path.display()));
        self.settings.last_dir = path.parent().map(PathBuf::from);
        self.source = Source::File(path);
        self.page = 0;
        self.page_count = None;
        self.print_all_pages = false;
        self.preview_dirty = true;
        self.status = None;
    }

    fn pick_file(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .add_filter(
                "Labels (PDF and images)",
                &[
                    "pdf", "png", "jpg", "jpeg", "bmp", "webp", "gif", "tif", "tiff",
                ],
            )
            .add_filter("PDF", &["pdf"])
            .add_filter("Images", &jobs::IMAGE_EXTENSIONS);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_file(path);
        }
    }

    /// Is the selected printer reachable right now?
    fn ready(&self) -> bool {
        match &self.selected {
            Some(Selected::Wired { spec, .. }) => self.find_printer(spec).is_some_and(|p| p.online),
            Some(Selected::Bluetooth { id, .. }) => self
                .ble_state
                .connected
                .as_ref()
                .is_some_and(|(connected, _)| connected == id),
            None => false,
        }
    }

    fn find_printer(&self, spec: &str) -> Option<&DiscoveredPrinter> {
        self.discovery
            .as_ref()?
            .printers
            .iter()
            .find(|p| p.target_spec().eq_ignore_ascii_case(spec))
    }

    fn destination(&self) -> Option<Destination> {
        match &self.selected {
            Some(Selected::Wired { spec, .. }) => Some(Destination::Wired(spec.clone())),
            Some(Selected::Bluetooth { .. }) => Some(Destination::Bluetooth),
            None => None,
        }
    }

    fn select_wired(&mut self, printer: &DiscoveredPrinter, automatic: bool) {
        let spec = printer.target_spec();
        self.selected = Some(Selected::Wired {
            spec: spec.clone(),
            name: printer.name.clone(),
            kind: printer.kind,
        });
        self.settings.last_printer = Some(spec);
        self.save_now = true;
        if automatic {
            self.ok(format!(
                "Connected to {} ({})",
                printer.name,
                printer.kind.label()
            ));
        } else {
            self.log(format!(
                "selected {} ({})",
                printer.name,
                printer.target_spec()
            ));
            self.status = None;
        }
    }

    // ------------------------------------------------------------ requests

    fn request_preview(&mut self) {
        self.preview_dirty = false;
        // Advance first, so a preview still in flight is ignored.
        self.generation += 1;
        if self.source == Source::Empty {
            self.rendering = false;
            self.preview = None;
            self.preview_error = None;
            self.page_count = None;
            return;
        }
        self.rendering = true;
        let _ = self.jobs.requests.send(JobRequest::Preview {
            generation: self.generation,
            source: self.source.clone(),
            page: self.page,
            layout: self.layout(),
        });
    }

    fn print(&mut self) {
        if self.busy.is_some() {
            return;
        }
        if self.source == Source::Empty {
            self.fail(jobs::NOTHING_OPEN);
            return;
        }
        let Some(destination) = self.destination() else {
            self.fail("Choose a printer first");
            return;
        };
        if !self.ready() {
            self.fail("The printer is not connected");
            return;
        }
        if let Err(error) = self.config().validate() {
            self.fail(error.to_string());
            return;
        }
        let pages = if self.print_all_pages && self.page_count.unwrap_or(1) > 1 {
            Pages::All
        } else {
            Pages::One(self.page)
        };
        self.busy = Some("Preparing…".to_string());
        self.status = None;
        let _ = self.jobs.requests.send(JobRequest::Print {
            source: self.source.clone(),
            pages,
            layout: self.layout(),
            config: self.config(),
            destination,
        });
    }

    fn send_raw(&mut self, what: &'static str, bytes: Vec<u8>) {
        let Some(destination) = self.destination().filter(|_| self.ready()) else {
            self.fail("Connect a printer first");
            return;
        };
        self.busy = Some(format!("Sending {what}…"));
        let _ = self.jobs.requests.send(JobRequest::Raw {
            what,
            bytes,
            destination,
        });
    }

    // -------------------------------------------------------------- events

    fn poll(&mut self) {
        while let Ok(discovery) = self.watcher.updates.try_recv() {
            self.on_discovery(discovery);
        }
        while let Ok(event) = self.jobs.events.try_recv() {
            self.on_job(event);
        }
        while let Ok(event) = self.ble.events.try_recv() {
            self.on_ble(event);
        }
        self.maybe_reconnect_ble();
    }

    fn on_discovery(&mut self, discovery: Discovery) {
        for warning in &discovery.warnings {
            self.log(format!("warning: {warning}"));
        }
        let first = self.discovery.is_none();
        self.discovery = Some(discovery);

        // Report a selected wired printer going away or coming back.
        if let Some(Selected::Wired { spec, name, .. }) = self.selected.clone() {
            let online = self.find_printer(&spec).is_some_and(|p| p.online);
            if !online {
                self.log(format!("{name} disconnected"));
            }
        }

        if !self.settings.auto_connect {
            return;
        }
        let keep = match &self.selected {
            Some(Selected::Bluetooth { .. }) => self.ble_state.connected.is_some(),
            Some(Selected::Wired { .. }) => self.ready(),
            None => false,
        };
        if keep {
            return;
        }
        let best = self
            .discovery
            .as_ref()
            .and_then(|d| best_printer(&d.printers, self.settings.last_printer.as_deref()))
            .cloned();
        match best {
            Some(printer) => self.select_wired(&printer, true),
            None if first => self.log("no wired label printer found"),
            None => {}
        }
    }

    fn maybe_reconnect_ble(&mut self) {
        if !self.settings.auto_connect
            || self.discovery.is_none()
            || self.ready()
            || self.ble_state.connected.is_some()
            || self.ble_state.connecting.is_some()
            || self.ble_state.scanning
        {
            return;
        }
        let Some(remembered) = self.settings.ble_printer.clone() else {
            return;
        };
        let wait = reconnect_delay(self.ble_state.auto_attempts);
        if self
            .ble_state
            .last_auto_attempt
            .is_some_and(|at| at.elapsed() < wait)
        {
            return;
        }
        self.ble_state.last_auto_attempt = Some(Instant::now());
        self.ble_state.auto_attempts += 1;
        self.ble_state.connecting = Some(remembered.name.clone());
        if self.ble_state.auto_attempts == 1 {
            self.log(format!("looking for {} over Bluetooth…", remembered.name));
        }
        let _ = self.ble.commands.send(BleCommand::Reconnect {
            id: remembered.id,
            name: remembered.name,
        });
    }

    fn on_job(&mut self, event: JobEvent) {
        match event {
            JobEvent::Preview {
                generation,
                frame,
                pages,
            } => {
                if generation != self.generation {
                    return;
                }
                self.rendering = false;
                self.preview_error = None;
                if pages != self.page_count {
                    if let Some(count) = pages {
                        self.log(format!("PDF has {count} page(s)"));
                    }
                    self.page_count = pages;
                }
                let image = egui::ColorImage::from_gray(
                    [frame.width() as usize, frame.height() as usize],
                    frame.gray(),
                );
                match &mut self.preview {
                    Some(texture) => texture.set(image, PREVIEW_TEXTURE),
                    None => {
                        self.preview = Some(self.ctx.load_texture(
                            "label-preview",
                            image,
                            PREVIEW_TEXTURE,
                        ));
                    }
                }
            }
            JobEvent::PreviewFailed {
                generation,
                message,
            } => {
                if generation != self.generation {
                    return;
                }
                self.rendering = false;
                self.log(format!("preview: {message}"));
                self.preview_error = Some(message);
                self.preview = None;
            }
            JobEvent::Rendering(pages) => {
                self.busy = Some(if pages > 1 {
                    format!("Rendering {pages} pages…")
                } else {
                    "Rendering…".to_string()
                });
            }
            JobEvent::ForBluetooth { what, bytes } => {
                self.busy = Some(format!("Sending {what} over Bluetooth…"));
                self.ble_state.progress = Some((0, bytes.len()));
                self.ble.cancel.store(false, Ordering::SeqCst);
                self.ble_state.started = Some(Instant::now());
                let _ = self.ble.commands.send(BleCommand::Print {
                    job: bytes,
                    fast: self.settings.ble_fast,
                });
            }
            JobEvent::Printed {
                what,
                bytes,
                target,
            } => {
                self.busy = None;
                let name = match &self.selected {
                    Some(Selected::Wired { name, .. }) => name.clone(),
                    _ => target.clone(),
                };
                self.log(format!("sent {bytes} bytes to {target}"));
                self.ok(format!("Sent {what} to {name}"));
            }
            JobEvent::Failed(message) => {
                self.busy = None;
                self.fail(message);
            }
        }
    }

    fn on_ble(&mut self, event: BleEvent) {
        match event {
            BleEvent::Scanning => self.ble_state.scanning = true,
            BleEvent::Devices(devices) => {
                self.ble_state.scanning = false;
                let printers = devices
                    .iter()
                    .filter(|d| filter::looks_like_printer(&d.name))
                    .count();
                self.log(format!(
                    "Bluetooth scan: {} device(s), {printers} look like printers",
                    devices.len()
                ));
                // A scan, even a manual one, that sees the remembered printer
                // connects to it.
                let remembered = self.settings.ble_printer.clone();
                if let Some(remembered) = remembered.filter(|_| {
                    self.settings.auto_connect
                        && self.ble_state.connected.is_none()
                        && self.ble_state.connecting.is_none()
                }) {
                    if let Some(id) =
                        ble::find_remembered(&devices, &remembered.id, &remembered.name)
                    {
                        self.log(format!("found {}, connecting…", remembered.name));
                        self.ble_state.connecting = Some(remembered.name);
                        let _ = self.ble.commands.send(BleCommand::Connect(id));
                    }
                }
                self.ble_state.devices = devices;
            }
            BleEvent::NotFound { name, detail } => {
                // Not an error: the printer may be off. Keep looking quietly.
                self.ble_state.scanning = false;
                self.ble_state.connecting = None;
                let next = reconnect_delay(self.ble_state.auto_attempts);
                self.log(format!(
                    "{name}: {detail}; trying again in {}",
                    short_duration(next)
                ));
            }
            BleEvent::Connected { id, name, channel } => {
                self.ble_state.scanning = false;
                self.ble_state.connecting = None;
                self.ble_state.auto_attempts = 0;
                self.ble_state.last_auto_attempt = None;
                self.ble_state.connected = Some((id.clone(), name.clone()));
                self.log(format!("Bluetooth: {name} print channel {channel}"));
                self.save_now = true;
                self.settings.ble_printer = Some(RememberedBle {
                    id: id.clone(),
                    name: name.clone(),
                });
                self.selected = Some(Selected::Bluetooth {
                    id,
                    name: name.clone(),
                });
                self.ok(format!("Connected to {name} over Bluetooth"));
            }
            BleEvent::Progress { sent, total } => self.ble_state.progress = Some((sent, total)),
            BleEvent::Printed(bytes) => {
                self.busy = None;
                self.ble_state.progress = None;
                let took = self
                    .ble_state
                    .started
                    .take()
                    .map_or_else(String::new, |at| {
                        format!(" in {}", short_duration(at.elapsed()))
                    });
                let mode = if self.settings.ble_fast {
                    "fast"
                } else {
                    "reliable"
                };
                self.log(format!(
                    "sent {bytes} bytes over Bluetooth{took} ({mode} mode)"
                ));
                self.ok(format!("Sent to the printer over Bluetooth{took}"));
            }
            BleEvent::Cancelled => {
                self.busy = None;
                self.ble_state.progress = None;
                self.fail(
                    "Printing cancelled; the printer may need a Feed to clear the partial label",
                );
            }
            BleEvent::Disconnected => {
                self.ble_state.connected = None;
                self.ble_state.connecting = None;
                if matches!(self.selected, Some(Selected::Bluetooth { .. })) {
                    self.selected = None;
                }
                self.log("Bluetooth disconnected");
            }
            BleEvent::Error(message) => {
                self.ble_state.scanning = false;
                self.ble_state.connecting = None;
                if self.ble_state.progress.take().is_some() {
                    self.busy = None;
                }
                self.fail(message);
            }
        }
    }
}

impl eframe::App for ThermalApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.save_now) {
            if let Some(storage) = frame.storage_mut() {
                eframe::set_value(storage, settings::STORAGE_KEY, &self.settings);
                storage.flush();
            }
        }
        let ctx = ui.ctx().clone();

        self.poll();
        self.handle_input(&ctx);
        if self.preview_dirty {
            self.request_preview();
        }

        egui::Panel::top("header")
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(20, 14)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.header(ui));
        egui::Panel::bottom("status")
            .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(20, 8)))
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::right("side")
            .resizable(false)
            .exact_size(350.0)
            .frame(Frame::new().fill(BG).inner_margin(Margin {
                left: 8,
                right: 20,
                top: 0,
                bottom: 12,
            }))
            .show_separator_line(false)
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt("side-scroll")
                    .auto_shrink(false)
                    .show(ui, |ui| self.side(ui));
            });
        egui::CentralPanel::default_margins()
            .frame(Frame::new().fill(BG).inner_margin(Margin {
                left: 20,
                right: 8,
                top: 0,
                bottom: 12,
            }))
            .show(ui, |ui| self.main(ui));

        self.drop_overlay(&ctx);
        self.debug_screenshot(&ctx);
        if self.busy.is_some() || self.rendering {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, settings::STORAGE_KEY, &self.settings);
    }
}

impl ThermalApp {
    fn handle_input(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .find_map(|f| Some(f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()))
        });
        if let Some(path) = dropped {
            self.open_file(path);
        }
        let (print, open) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(Key::P),
                i.modifiers.command && i.key_pressed(Key::O),
            )
        });
        if print {
            self.print();
        }
        if open {
            self.pick_file();
        }
    }

    // --------------------------------------------------------------- header

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let logo = self.logo.get_or_insert_with(|| {
                let image =
                    egui::ColorImage::from_rgba_unmultiplied([64, 64], &crate::icon::icon_rgba(64));
                ui.ctx()
                    .load_texture("logo", image, egui::TextureOptions::LINEAR)
            });
            ui.add(egui::Image::new((logo.id(), Vec2::splat(34.0))));
            ui.vertical(|ui| {
                ui.label(RichText::new("Label Printer").size(21.0).color(TEXT));
                ui.label(
                    RichText::new("Label printing, no driver")
                        .size(13.0)
                        .color(MUTED),
                );
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ready = self.ready() && self.busy.is_none();
                let label = match (&self.busy, self.ble_state.progress) {
                    (_, Some((sent, total))) if total > 0 => {
                        format!("{} Sending {}%", icon::PRINTER, sent * 100 / total)
                    }
                    (Some(_), _) => format!("{} Printing…", icon::PRINTER),
                    (None, _) => format!("{} Print", icon::PRINTER),
                };
                let hover = if self.ready() {
                    "Print (Ctrl+P)"
                } else {
                    "Connect a printer first"
                };
                if theme::big_accent_button(ui, &label, ready)
                    .on_hover_text(hover)
                    .on_disabled_hover_text(hover)
                    .clicked()
                {
                    self.print();
                }
                ui.add(
                    DragValue::new(&mut self.settings.copies)
                        .range(1..=99)
                        .suffix(" ×"),
                )
                .on_hover_text("Copies");
                ui.add_space(8.0);
                self.connection_chip(ui);
            });
        });
    }

    fn connection_chip(&self, ui: &mut Ui) {
        let (glyph, text, color) = match &self.selected {
            Some(Selected::Wired { name, kind, .. }) => {
                let glyph = match kind {
                    PrinterKind::Usb => icon::USB,
                    PrinterKind::Queue => icon::QUEUE,
                    PrinterKind::BluetoothSerial => icon::BLUETOOTH,
                    PrinterKind::Serial => icon::PLUG,
                };
                if self.ready() {
                    (glyph, format!("{name} · ready"), GOOD)
                } else {
                    (icon::LINK_BREAK, format!("{name} · not connected"), WARN)
                }
            }
            Some(Selected::Bluetooth { name, .. }) => {
                if self.ready() {
                    (icon::BLUETOOTH_CONNECTED, format!("{name} · ready"), GOOD)
                } else {
                    (icon::LINK_BREAK, format!("{name} · not connected"), WARN)
                }
            }
            None => {
                let looking = self.ble_state.connecting.clone().or_else(|| {
                    self.settings
                        .ble_printer
                        .as_ref()
                        .filter(|_| self.settings.auto_connect)
                        .map(|p| p.name.clone())
                });
                match looking {
                    Some(name) => (icon::BLUETOOTH, format!("Looking for {name}…"), MUTED),
                    None => (icon::LINK_BREAK, "No printer".to_string(), MUTED),
                }
            }
        };
        theme::chip(ui, glyph, &text, color);
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some((sent, total)) = self.ble_state.progress {
                let fraction = if total == 0 {
                    0.0
                } else {
                    sent as f32 / total as f32
                };
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(220.0)
                        .text(format!("{} / {} KB", sent / 1024, total / 1024)),
                );
                if ui.button(format!("{} Cancel", icon::STOP)).clicked() {
                    self.ble.cancel.store(true, Ordering::SeqCst);
                }
                return;
            }
            if let Some(busy) = &self.busy {
                ui.spinner();
                ui.label(RichText::new(busy).color(MUTED));
                return;
            }
            match &self.status {
                Some(Ok(message)) => {
                    ui.label(
                        RichText::new(format!("{} {message}", icon::CHECK_CIRCLE)).color(GOOD),
                    );
                }
                Some(Err(message)) => {
                    ui.label(
                        RichText::new(format!("{} {message}", icon::WARNING_CIRCLE)).color(BAD),
                    );
                }
                None => {
                    ui.label(
                        RichText::new(
                            "Drop a PDF or image anywhere, or press Ctrl+O. Ctrl+P prints.",
                        )
                        .size(12.0)
                        .color(MUTED),
                    );
                }
            }
        });
    }

    // ----------------------------------------------------------------- main

    /// Back to the empty drop zone.
    fn close_source(&mut self) {
        self.source = Source::Empty;
        self.page = 0;
        self.page_count = None;
        self.print_all_pages = false;
        self.preview_dirty = true;
    }

    fn main(&mut self, ui: &mut Ui) {
        if self.source == Source::Empty {
            // The drop zone has its own Choose button; no toolbar needed.
            self.drop_zone(ui);
            return;
        }
        ui.horizontal(|ui| {
            if theme::accent_button(ui, &format!("{} Open…", icon::FOLDER_OPEN))
                .on_hover_text("Ctrl+O")
                .clicked()
            {
                self.pick_file();
            }
            match self.source.clone() {
                Source::Empty => {
                    ui.label(RichText::new("No label open").size(12.0).color(MUTED));
                }
                Source::TestCard => {
                    theme::chip(ui, icon::BARCODE, "Test card", MUTED);
                    if theme::icon_button(ui, icon::X, "Close the test card").clicked() {
                        self.close_source();
                    }
                }
                Source::File(path) => {
                    let glyph = if jobs::is_pdf(&path) {
                        icon::FILE_PDF
                    } else {
                        icon::IMAGE
                    };
                    theme::chip(ui, glyph, &jobs::display_name(&path), TEXT);
                    if theme::icon_button(ui, icon::X, "Close").clicked() {
                        self.close_source();
                    }
                }
            }
            if self.rendering {
                ui.spinner();
            }
        });

        if let Some(count) = self.page_count.filter(|&count| count > 1) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.page > 0, egui::Button::new(icon::CARET_LEFT))
                    .on_hover_text("Previous page")
                    .clicked()
                {
                    self.page -= 1;
                    self.preview_dirty = true;
                }
                ui.label(RichText::new(format!("Page {} of {count}", self.page + 1)).color(TEXT));
                if ui
                    .add_enabled(self.page + 1 < count, egui::Button::new(icon::CARET_RIGHT))
                    .on_hover_text("Next page")
                    .clicked()
                {
                    self.page += 1;
                    self.preview_dirty = true;
                }
                ui.add_space(12.0);
                ui.label(RichText::new("Print").color(MUTED));
                let mut all = self.print_all_pages;
                theme::segmented(ui, &mut all, &[(false, "This page"), (true, "All pages")]);
                self.print_all_pages = all;
            });
        }

        ui.add_space(10.0);
        self.preview_area(ui);
    }

    /// The empty state: a large drop target that also opens the file picker.
    fn drop_zone(&mut self, ui: &mut Ui) {
        let size = ui.available_size().max(Vec2::splat(120.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Open a label file")
        });
        let dragging = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let active = response.hovered() || dragging;

        let painter = ui.painter_at(rect);
        // Hover lights the border; a file being dragged in also tints the zone.
        painter.rect_filled(
            rect,
            12,
            if dragging {
                theme::SELECTED
            } else {
                theme::CARD
            },
        );
        let edge = rect.shrink(1.5);
        let stroke = Stroke::new(
            1.5,
            if active {
                ACCENT
            } else {
                MUTED.gamma_multiply(0.5)
            },
        );
        let corners = [
            edge.left_top(),
            edge.right_top(),
            edge.right_bottom(),
            edge.left_bottom(),
            edge.left_top(),
        ];
        for side in corners.windows(2) {
            painter.extend(egui::Shape::dashed_line(side, stroke, 10.0, 7.0));
        }

        let column = egui::Rect::from_center_size(
            rect.center(),
            Vec2::new(rect.width().min(440.0) - 32.0, 250.0),
        );
        let mut open = response.clicked();
        let mut test_card = false;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(column)
                .layout(Layout::top_down(Align::Center)),
            |ui| {
                ui.label(
                    RichText::new(icon::FILE_ARROW_UP)
                        .size(58.0)
                        .color(if active { ACCENT } else { MUTED }),
                );
                ui.add_space(6.0);
                ui.label(RichText::new("Drop a label here").size(21.0).color(TEXT));
                ui.add_space(2.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "PDF or image. Shipping labels on a Letter or A4 page are found \
                             and cropped to fit the label.",
                        )
                        .size(12.5)
                        .color(MUTED),
                    )
                    .wrap(),
                );
                ui.add_space(14.0);
                open |= theme::accent_button(ui, &format!("{} Choose a file…", icon::FOLDER_OPEN))
                    .on_hover_text("Ctrl+O")
                    .clicked();
                ui.add_space(4.0);
                test_card = ui
                    .link(
                        RichText::new("or print a test card")
                            .size(12.0)
                            .color(MUTED),
                    )
                    .on_hover_text("Checks the printer, darkness and alignment")
                    .clicked();
            },
        );

        if test_card {
            self.source = Source::TestCard;
            self.preview_dirty = true;
        } else if open {
            self.pick_file();
        }
    }

    fn preview_area(&mut self, ui: &mut Ui) {
        if self.source == Source::Empty {
            self.drop_zone(ui);
            return;
        }
        let label = self.label_px();
        let available = ui.available_size() - Vec2::new(0.0, 22.0);
        let (rect, _) = ui.allocate_exact_size(available.max(Vec2::splat(50.0)), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect(
            rect,
            10,
            theme::CARD,
            Stroke::new(1.0, BORDER),
            StrokeKind::Inside,
        );

        let aspect = label.width as f32 / label.height as f32;
        let inner = rect.shrink(24.0);
        let mut size = Vec2::new(inner.width(), inner.width() / aspect);
        if size.y > inner.height() {
            size = Vec2::new(inner.height() * aspect, inner.height());
        }
        let paper = egui::Rect::from_center_size(inner.center(), size);

        // Shadow, then the label itself.
        painter.rect_filled(
            paper.translate(Vec2::new(0.0, 4.0)).expand(2.0),
            8,
            Color32::from_black_alpha(90),
        );
        painter.rect_filled(paper, 6, Color32::WHITE);

        match (&self.preview, &self.preview_error) {
            (_, Some(error)) => {
                painter.text(
                    paper.center(),
                    Align2::CENTER_CENTER,
                    format!("{}\n{error}", icon::WARNING_CIRCLE),
                    FontId::proportional(14.0),
                    Color32::from_rgb(0xB9, 0x1C, 0x1C),
                );
            }
            (Some(texture), None) => {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(texture.id(), paper.shrink(1.0), uv, Color32::WHITE);
            }
            (None, None) => {}
        }

        if let Some((sent, total)) = self.ble_state.progress {
            self.send_progress_card(ui, paper, sent, total);
        }

        ui.label(
            RichText::new(format!(
                "{:.1} × {:.1} mm · {} × {} dots at {DEFAULT_DPI} dpi · exactly what prints",
                self.settings.width_mm, self.settings.height_mm, label.width, label.height
            ))
            .size(11.5)
            .color(MUTED),
        );
    }

    /// Large progress card over the preview while a Bluetooth job is sending.
    fn send_progress_card(&mut self, ui: &mut Ui, over: egui::Rect, sent: usize, total: usize) {
        let painter = ui.painter_at(over);
        painter.rect_filled(over, 6, Color32::from_black_alpha(150));

        let width = over.width().clamp(200.0, 340.0);
        let card_rect = egui::Rect::from_center_size(over.center(), Vec2::new(width, 150.0));
        let fraction = if total == 0 {
            0.0
        } else {
            sent as f32 / total as f32
        };
        let elapsed = self.ble_state.started.map(|at| at.elapsed());
        let printer = match &self.selected {
            Some(Selected::Bluetooth { name, .. }) => name.clone(),
            _ => "the printer".to_string(),
        };

        ui.scope_builder(egui::UiBuilder::new().max_rect(card_rect), |ui| {
            theme::card(ui, true, |ui| {
                ui.label(
                    RichText::new(format!("{} Sending to {printer}", icon::BLUETOOTH))
                        .size(14.5)
                        .color(TEXT),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .fill(ACCENT)
                        .desired_height(14.0)
                        .text(format!("{:.0}%", fraction * 100.0)),
                );
                ui.add_space(4.0);
                let remaining = elapsed
                    .and_then(|elapsed| time_left(sent, total, elapsed))
                    .map_or_else(
                        || "estimating…".to_string(),
                        |left| format!("about {} left", short_duration(left)),
                    );
                ui.label(
                    RichText::new(format!(
                        "{} of {} KB · {remaining}",
                        sent / 1024,
                        total.div_ceil(1024)
                    ))
                    .size(12.0)
                    .color(MUTED),
                );
                ui.add_space(6.0);
                if ui.button(format!("{} Cancel", icon::STOP)).clicked() {
                    self.ble.cancel.store(true, Ordering::SeqCst);
                }
            });
        });
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop"),
        ));
        let rect = ctx.content_rect();
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(170));
        painter.rect_stroke(
            rect.shrink(16.0),
            14,
            Stroke::new(2.0, ACCENT),
            StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            format!("{}  Drop a PDF or image to print", icon::ARROW_LINE_DOWN),
            FontId::proportional(26.0),
            Color32::WHITE,
        );
    }

    // ----------------------------------------------------------------- side

    fn side(&mut self, ui: &mut Ui) {
        self.printer_section(ui);
        self.bluetooth_section(ui);
        self.label_section(ui);
        self.content_section(ui);
        self.quality_section(ui);
        self.tools_section(ui);
        self.log_section(ui);
    }

    fn printer_section(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Printer").size(15.0).color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Look for printers now").clicked()
                {
                    let _ = self.watcher.refresh.send(());
                    self.log("looking for printers…");
                }
            });
        });
        ui.add_space(6.0);

        let printers: Vec<DiscoveredPrinter> = self
            .discovery
            .as_ref()
            .map(|d| d.printers.clone())
            .unwrap_or_default();
        let shown: Vec<&DiscoveredPrinter> = printers
            .iter()
            .filter(|p| self.show_all_printers || !p.noise)
            .collect();

        if self.discovery.is_none() {
            theme::card(ui, false, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Looking for printers…").color(MUTED));
                });
            });
        } else if shown.is_empty() {
            theme::card(ui, false, |ui| {
                ui.label(RichText::new("No label printer found").color(TEXT));
                theme::hint(
                    ui,
                    "Plug the printer in with USB and switch it on. It is found by itself, \
                     with no driver to install. Or use Bluetooth below.",
                );
            });
        }

        for printer in shown {
            let spec = printer.target_spec();
            let selected =
                matches!(&self.selected, Some(Selected::Wired { spec: s, .. }) if *s == spec);
            let clicked = theme::card(ui, selected, |ui| {
                let response = ui
                    .horizontal(|ui| {
                        let glyph = match printer.kind {
                            PrinterKind::Usb => icon::USB,
                            PrinterKind::Queue => icon::QUEUE,
                            PrinterKind::BluetoothSerial => icon::BLUETOOTH,
                            PrinterKind::Serial => icon::PLUG,
                        };
                        ui.label(RichText::new(glyph).size(20.0).color(if selected {
                            ACCENT
                        } else {
                            MUTED
                        }));
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                theme::dot(ui, if printer.online { GOOD } else { MUTED });
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&printer.name).size(14.5).color(TEXT),
                                    )
                                    .truncate(),
                                );
                            });
                            let state = if printer.online { "" } else { " · offline" };
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{}{state}", printer.kind.label()))
                                        .size(12.0)
                                        .color(MUTED),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&printer.detail).size(11.0).color(MUTED),
                                )
                                .truncate(),
                            );
                        });
                    })
                    .response;
                ui.interact(
                    response.rect,
                    ui.id().with(("printer", &spec)),
                    Sense::click(),
                )
                .on_hover_text(&spec)
                .clicked()
            });
            if clicked {
                self.select_wired(printer, false);
            }
            ui.add_space(6.0);
        }

        let hidden = printers.iter().filter(|p| p.noise).count();
        ui.horizontal(|ui| {
            theme::toggle_row(ui, &mut self.settings.auto_connect, "Connect automatically")
                .on_hover_text(
                    "Pick the label printer by itself on start and when one is plugged in",
                );
        });
        if hidden > 0 {
            ui.checkbox(
                &mut self.show_all_printers,
                format!("Show {hidden} other device(s): PDF queues, serial ports"),
            );
        }
    }

    fn bluetooth_section(&mut self, ui: &mut Ui) {
        theme::section(ui, "Bluetooth");
        theme::card(
            ui,
            matches!(self.selected, Some(Selected::Bluetooth { .. })),
            |ui| {
                ui.horizontal(|ui| {
                    let scanning = self.ble_state.scanning;
                    if ui
                        .add_enabled(
                            !scanning,
                            egui::Button::new(format!("{} Scan", icon::MAGNIFYING_GLASS)),
                        )
                        .clicked()
                    {
                        self.ble_state.devices.clear();
                        self.ble_state.scanning = true;
                        let _ = self.ble.commands.send(BleCommand::Scan);
                    }
                    if scanning || self.ble_state.connecting.is_some() {
                        ui.spinner();
                    }
                    if let Some((_, name)) = self.ble_state.connected.clone() {
                        ui.label(
                            RichText::new(format!("{} {name}", icon::BLUETOOTH_CONNECTED))
                                .color(GOOD),
                        );
                        if ui.small_button("Disconnect").clicked() {
                            let _ = self.ble.commands.send(BleCommand::Disconnect);
                            self.settings.ble_printer = None;
                            self.save_now = true;
                        }
                    }
                });
                theme::hint(
                    ui,
                    "Connects over Bluetooth LE, like the phone app: no pairing and no PIN. \
                 Don't add the printer in Windows Bluetooth settings.",
                );
                ui.add_space(4.0);
                theme::toggle_row(ui, &mut self.settings.ble_fast, "Faster sending").on_hover_text(
                    "Streams data without waiting for the printer to confirm every \
                         chunk. Turn off if labels come out garbled or cut short.",
                );

                if !self.ble_state.devices.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.ble_state.filter)
                                .hint_text("Filter, e.g. BY-482")
                                .desired_width(150.0),
                        );
                        ui.checkbox(&mut self.ble_state.show_all, "Show all");
                    });
                    let mut devices: Vec<DeviceInfo> = self
                        .ble_state
                        .devices
                        .iter()
                        .filter(|d| {
                            filter::is_visible(
                                &d.name,
                                &self.ble_state.filter,
                                self.ble_state.show_all,
                            )
                        })
                        .cloned()
                        .collect();
                    devices.sort_by_key(|d| filter::sort_key(&d.name));
                    let hidden = self.ble_state.devices.len() - devices.len();
                    for device in devices {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::BLUETOOTH).color(MUTED));
                            ui.add(
                                egui::Label::new(RichText::new(&device.name).color(TEXT))
                                    .truncate(),
                            );
                            if let Some(rssi) = device.rssi {
                                ui.label(
                                    RichText::new(format!("{rssi} dBm")).size(11.0).color(MUTED),
                                );
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("Connect").clicked() {
                                    self.ble_state.connecting = Some(device.name.clone());
                                    self.log(format!("connecting to {}…", device.name));
                                    let _ = self
                                        .ble
                                        .commands
                                        .send(BleCommand::Connect(device.id.clone()));
                                }
                            });
                        });
                    }
                    if hidden > 0 {
                        theme::hint(ui, &format!("{hidden} non-printer device(s) hidden"));
                    }
                }
            },
        );
    }

    fn label_section(&mut self, ui: &mut Ui) {
        theme::section(ui, "Label");
        theme::card(ui, false, |ui| {
            let mut changed = false;
            let current = self.settings.preset().map_or("Custom size", |p| p.name);
            ComboBox::from_id_salt("preset")
                .width(ui.available_width())
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for preset in PRESETS {
                        if ui
                            .selectable_label(current == preset.name, preset.name)
                            .clicked()
                        {
                            self.settings.width_mm = preset.width_mm;
                            self.settings.height_mm = preset.height_mm;
                            self.settings.gap_mm = preset.gap_mm;
                            changed = true;
                        }
                    }
                });
            egui::Grid::new("label-size")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Width").color(MUTED));
                    changed |= ui
                        .add(
                            DragValue::new(&mut self.settings.width_mm)
                                .range(5.0..=108.0)
                                .speed(0.1)
                                .suffix(" mm"),
                        )
                        .changed();
                    ui.end_row();
                    ui.label(RichText::new("Height").color(MUTED));
                    changed |= ui
                        .add(
                            DragValue::new(&mut self.settings.height_mm)
                                .range(5.0..=1000.0)
                                .speed(0.1)
                                .suffix(" mm"),
                        )
                        .changed();
                    ui.end_row();
                    ui.label(RichText::new("Gap").color(MUTED));
                    ui.add(
                        DragValue::new(&mut self.settings.gap_mm)
                            .range(0.0..=20.0)
                            .speed(0.1)
                            .suffix(" mm"),
                    )
                    .on_hover_text("Space between labels; 0 for continuous rolls");
                    ui.end_row();
                });
            if changed {
                self.preview_dirty = true;
            }
        });
    }

    fn content_section(&mut self, ui: &mut Ui) {
        theme::section(ui, "Content");
        theme::card(ui, false, |ui| {
            let options = &mut self.settings.options;
            let mut changed = false;
            changed |= theme::toggle_row(ui, &mut options.auto_crop, "Trim blank margins")
                .on_hover_text(
                    "Finds the label on a full page, e.g. a 4×6 shipping label on Letter paper",
                )
                .changed();
            ui.add_space(6.0);
            ui.label(RichText::new("Rotate").color(MUTED));
            let choices: Vec<(Rotation, &str)> =
                Rotation::ALL.iter().map(|r| (*r, r.label())).collect();
            changed |= theme::segmented(ui, &mut options.rotation, &choices);
            ui.add_space(6.0);
            ui.label(RichText::new("Black and white").color(MUTED));
            let choices: Vec<(Halftone, &str)> =
                Halftone::ALL.iter().map(|h| (*h, h.label())).collect();
            changed |= theme::segmented(ui, &mut options.halftone, &choices);
            theme::hint(ui, "Auto keeps text and barcodes sharp and dithers photos.");
            ui.add_space(4.0);
            changed |= theme::toggle_row(ui, &mut options.invert, "Invert").changed();
            if changed {
                self.preview_dirty = true;
            }
        });
    }

    fn quality_section(&mut self, ui: &mut Ui) {
        theme::section(ui, "Print quality");
        theme::card(ui, false, |ui| {
            egui::Grid::new("quality")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Darkness").color(MUTED));
                    ui.add(Slider::new(&mut self.settings.density, 1..=15));
                    ui.end_row();
                    ui.label(RichText::new("Speed").color(MUTED));
                    ui.add(Slider::new(&mut self.settings.speed, 2..=6))
                        .on_hover_text("Slower is darker and sharper");
                    ui.end_row();
                    ui.label(RichText::new("Copies").color(MUTED));
                    ui.add(Slider::new(&mut self.settings.copies, 1..=99));
                    ui.end_row();
                });
        });
    }

    fn tools_section(&mut self, ui: &mut Ui) {
        theme::section(ui, "Printer tools");
        theme::card(ui, false, |ui| {
            let ready = self.ready() && self.busy.is_none();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(format!("{} Feed label", icon::ARROW_LINE_DOWN)),
                    )
                    .on_hover_text("Feed one blank label")
                    .clicked()
                {
                    self.send_raw("feed", feed_job());
                }
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(format!("{} Calibrate", icon::RULER)),
                    )
                    .on_hover_text("Measure label and gap with the sensor (feeds a few labels)")
                    .clicked()
                {
                    match calibrate_job(&self.config()) {
                        Ok(job) => self.send_raw("calibration", job),
                        Err(error) => self.fail(error.to_string()),
                    }
                }
            });
            theme::hint(
                ui,
                "Calibrate after changing label stock if prints drift or skip labels.",
            );
        });
    }

    fn log_section(&mut self, ui: &mut Ui) {
        ui.add_space(14.0);
        let title = format!("{} Activity ({})", icon::LIST_BULLETS, self.log.len());
        if ui
            .selectable_label(self.show_log, RichText::new(title).color(MUTED))
            .clicked()
        {
            self.show_log = !self.show_log;
        }
        if self.show_log {
            Frame::new()
                .fill(Color32::from_rgb(0x12, 0x13, 0x16))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ScrollArea::vertical()
                        .id_salt("log")
                        .max_height(220.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in &self.log {
                                ui.label(RichText::new(line).monospace().size(11.0).color(MUTED));
                            }
                        });
                });
        }
    }
}

impl ThermalApp {
    /// Debug hook behind `LABEL_PRINTER_SCREENSHOT`: once discovery and the
    /// preview have settled, capture the window to a PNG and close.
    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        let Some((path, frames)) = self.screenshot.as_mut() else {
            return;
        };
        let captured = ctx.input(|i| {
            i.raw.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = captured {
            let [w, h] = image.size;
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buffer) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
                if let Err(error) = buffer.save(&*path) {
                    eprintln!("screenshot {}: {error}", path.display());
                }
            }
            self.screenshot = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        *frames += 1;
        let settled = self.discovery.is_some()
            && !self.rendering
            && (self.preview.is_some() || self.source == Source::Empty);
        if settled && *frames > 30 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

/// Time left at the average rate so far, once there is enough to go on.
fn time_left(sent: usize, total: usize, elapsed: Duration) -> Option<Duration> {
    if sent == 0 || sent >= total || elapsed < Duration::from_millis(500) {
        return None;
    }
    let rate = sent as f64 / elapsed.as_secs_f64();
    Some(Duration::from_secs_f64((total - sent) as f64 / rate))
}

/// Wait before the next automatic Bluetooth reconnect: right away at first,
/// every 5 s for the first minute, then every 30 s.
fn reconnect_delay(attempts: u32) -> Duration {
    match attempts {
        0 => Duration::ZERO,
        1..=12 => Duration::from_secs(5),
        _ => Duration::from_secs(30),
    }
}

/// `42 s`, `1 min 5 s`.
fn short_duration(duration: Duration) -> String {
    let secs = duration.as_secs().max(1);
    if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{} min {} s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_uses_the_average_rate() {
        let left = time_left(25, 100, Duration::from_secs(5)).expect("estimate");
        assert_eq!(left.as_secs(), 15);
        // Not enough information, or already done.
        assert_eq!(time_left(0, 100, Duration::from_secs(5)), None);
        assert_eq!(time_left(10, 100, Duration::from_millis(100)), None);
        assert_eq!(time_left(100, 100, Duration::from_secs(5)), None);
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(short_duration(Duration::from_millis(300)), "1 s");
        assert_eq!(short_duration(Duration::from_secs(42)), "42 s");
        assert_eq!(short_duration(Duration::from_secs(65)), "1 min 5 s");
    }

    #[test]
    fn reconnects_back_off() {
        assert_eq!(reconnect_delay(0), Duration::ZERO);
        assert_eq!(reconnect_delay(1), Duration::from_secs(5));
        assert_eq!(reconnect_delay(12), Duration::from_secs(5));
        assert_eq!(reconnect_delay(13), Duration::from_secs(30));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_secs(30));
    }
}
