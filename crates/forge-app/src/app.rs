//! The application shell: state, file operations, clipboard, panels,
//! tabs, dialogs and autosave. Commands and menus live in `commands.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use forge_core::buffer::TiledBuffer;
use forge_core::document::Document;
use forge_core::filters::Filter;
use forge_core::selection;
use forge_core::styles::LayerStyle;
use forge_core::transform;
use forge_core::{composite, fill, Color, IRect, Layer, Rgba8};

use crate::actions::Recorder;
use crate::canvas::{self, DocTab};
use crate::commands::Cmd;
use crate::dialogs::{self, Outcome};
use crate::panels::{self, BrushPreset, LayerAction, LayersUi};
use crate::text::{self, FontEntry};
use crate::tools::ToolState;

/// Brand colours (from the logo).
pub const ACCENT: Color32 = Color32::from_rgb(198, 244, 50);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(78, 96, 30);
pub const INK: Color32 = Color32::from_rgb(24, 23, 28);
pub const PAPER: Color32 = Color32::from_rgb(238, 234, 226);

/// Name of the temporary layer used by Quick Mask mode.
pub const QUICK_MASK: &str = "Quick Mask";

pub enum Dialog {
    New(dialogs::NewDoc),
    ImageSize(dialogs::ImageSize),
    CanvasSize(dialogs::CanvasSize),
    Adjust(Box<dialogs::AdjustDialog>),
    Filter(Box<dialogs::FilterDialog>),
    Transform(Box<dialogs::TransformDialog>),
    Text(Box<dialogs::TextDialog>),
    Fill(dialogs::FillDialog),
    Stroke(dialogs::StrokeDialog),
    SelModify(dialogs::SelModify),
    LayerStyle(Box<dialogs::LayerStyleDialog>),
    ColorRange(dialogs::ColorRangeDialog),
    FocusArea(dialogs::FocusAreaDialog),
    Batch(dialogs::BatchDialog),
    NewGuide(dialogs::NewGuideDialog),
    Animation(dialogs::AnimationDialog),
    Recovery(Vec<PathBuf>),
    About,
    Shortcuts,
    Message(String, String),
    ConfirmClose { tab: usize, quit: bool },
}

pub struct Clip {
    pub pixels: TiledBuffer<Rgba8>,
    pub origin: IRect,
}

/// Right-hand panels, arranged in tabbed groups like Photoshop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PanelId {
    Color,
    Swatches,
    Gradients,
    Patterns,
    Properties,
    Adjustments,
    Info,
    Navigator,
    Histogram,
    Layers,
    Channels,
    Paths,
    History,
    Actions,
    Brushes,
}

impl PanelId {
    pub fn name(self) -> &'static str {
        match self {
            PanelId::Color => "Color",
            PanelId::Swatches => "Swatches",
            PanelId::Gradients => "Gradients",
            PanelId::Patterns => "Patterns",
            PanelId::Properties => "Properties",
            PanelId::Adjustments => "Adjustments",
            PanelId::Info => "Info",
            PanelId::Navigator => "Navigator",
            PanelId::Histogram => "Histogram",
            PanelId::Layers => "Layers",
            PanelId::Channels => "Channels",
            PanelId::Paths => "Paths",
            PanelId::History => "History",
            PanelId::Actions => "Actions",
            PanelId::Brushes => "Brushes",
        }
    }
}

pub const PANEL_GROUPS: [&[PanelId]; 4] = [
    &[PanelId::Color, PanelId::Swatches, PanelId::Gradients, PanelId::Patterns],
    &[PanelId::Properties, PanelId::Adjustments, PanelId::Info, PanelId::Navigator, PanelId::Histogram],
    &[PanelId::Layers, PanelId::Channels, PanelId::Paths],
    &[PanelId::History, PanelId::Actions, PanelId::Brushes],
];

/// Selected tab and collapsed state of each panel group.
pub struct Panels {
    pub tabs: [usize; 4],
    pub collapsed: [bool; 4],
}

impl Default for Panels {
    fn default() -> Self {
        Self { tabs: [0, 0, 0, 0], collapsed: [false; 4] }
    }
}

impl Panels {
    /// Brings a panel to the front of its group (Window menu, F-keys).
    pub fn focus(&mut self, id: PanelId) {
        for (g, group) in PANEL_GROUPS.iter().enumerate() {
            if let Some(i) = group.iter().position(|p| *p == id) {
                self.tabs[g] = i;
                self.collapsed[g] = false;
            }
        }
    }

    pub fn is_front(&self, id: PanelId) -> bool {
        PANEL_GROUPS.iter().enumerate().any(|(g, group)| group.get(self.tabs[g]) == Some(&id))
    }
}

fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("forge-design"))
}

fn load_recent() -> Vec<PathBuf> {
    config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("recent.json")).ok())
        .and_then(|s| serde_json::from_str::<Vec<PathBuf>>(&s).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.exists())
        .collect()
}

pub struct ForgeApp {
    pub tabs: Vec<DocTab>,
    pub active: usize,
    pub tools: ToolState,
    pub dialog: Option<Dialog>,
    layers_ui: LayersUi,
    editing_bg: bool,
    hex_buf: String,
    pub fonts: Vec<FontEntry>,
    pub clipboard: Option<Clip>,
    pub last_filter: Option<Filter>,
    pub panels: Panels,
    quit_confirmed: bool,
    title: String,
    pub recorder: Recorder,
    pub style_clipboard: Option<LayerStyle>,
    pub brush_presets: Vec<BrushPreset>,
    pub swatches: Vec<Color>,
    pub fullscreen: bool,
    pub hide_panels: bool,
    pub light_theme: bool,
    last_autosave: Instant,
    pub recent: Vec<PathBuf>,
    pub updater: crate::updater::Updater,
}

fn recovery_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("forge-design")
        .join("recovery")
}

impl ForgeApp {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        setup_style(&cc.egui_ctx, false);
        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            tools: ToolState::default(),
            dialog: None,
            layers_ui: LayersUi::default(),
            editing_bg: false,
            hex_buf: String::new(),
            fonts: text::discover_fonts(),
            clipboard: None,
            last_filter: None,
            panels: Panels::default(),
            quit_confirmed: false,
            title: String::new(),
            recorder: Recorder::load(),
            style_clipboard: None,
            brush_presets: panels::default_presets(),
            swatches: Vec::new(),
            fullscreen: false,
            hide_panels: false,
            light_theme: false,
            last_autosave: Instant::now(),
            recent: load_recent(),
            updater: crate::updater::Updater::default(),
        };
        // Quietly look for a newer release on GitHub at startup.
        app.updater.check(true);
        for f in files {
            app.open_path(&f);
        }
        // Offer to recover documents from a crash.
        if let Ok(rd) = std::fs::read_dir(recovery_dir()) {
            let found: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "ora")).collect();
            if !found.is_empty() {
                app.dialog = Some(Dialog::Recovery(found));
            }
        }
        app
    }

    pub fn add_tab(&mut self, doc: Document) {
        self.tabs.push(DocTab::new(doc));
        self.active = self.tabs.len() - 1;
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.dialog = Some(Dialog::Message("Error".into(), msg.into()));
    }

    pub fn open_path(&mut self, path: &Path) {
        if crate::text::is_font_file(path) {
            return self.add_font(path);
        }
        match forge_io::open(path) {
            Ok(doc) => {
                self.add_tab(doc);
                self.remember(path);
            }
            Err(e) => self.error(format!("Could not open {}:\n{e}", path.display())),
        }
    }

    /// Adds a file to Recent Projects.
    pub fn remember(&mut self, path: &Path) {
        let p = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.recent.retain(|r| *r != p);
        self.recent.insert(0, p);
        self.recent.truncate(20);
        if let Some(d) = config_dir() {
            let _ = std::fs::create_dir_all(&d);
            if let Ok(s) = serde_json::to_string_pretty(&self.recent) {
                let _ = std::fs::write(d.join("recent.json"), s);
            }
        }
    }

    /// Installs a .ttf/.otf font (Type > Add Font, or drag & drop).
    pub fn add_font(&mut self, path: &Path) {
        match crate::text::install_font(path) {
            Ok(entry) => {
                self.tools.status = format!("Font installed: {}", entry.name);
                self.fonts = crate::text::discover_fonts();
                if let Some(i) = self.fonts.iter().position(|f| f.path == entry.path) {
                    self.tools.font_index = i;
                }
            }
            Err(e) => self.error(format!("Could not add font {}:\n{e}", path.display())),
        }
    }

    pub fn add_font_dialog(&mut self) {
        let files = rfd::FileDialog::new().add_filter("Fonts", &["ttf", "otf"]).pick_files();
        for f in files.unwrap_or_default() {
            self.add_font(&f);
        }
    }

    // ------------------------------------------------------------ file ops

    pub fn open_dialog(&mut self) {
        let files = rfd::FileDialog::new()
            .add_filter("All supported images", forge_io::OPEN_EXTENSIONS)
            .add_filter("OpenRaster", &["ora"])
            .add_filter("Photoshop", &["psd"])
            .pick_files();
        for f in files.unwrap_or_default() {
            self.open_path(&f);
        }
    }

    pub fn save(&mut self, save_as: bool) -> bool {
        let Some(tab) = self.tabs.get_mut(self.active) else { return false };
        let path = match (&tab.doc.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let stem = tab.doc.name.rsplit_once('.').map_or(tab.doc.name.as_str(), |(s, _)| s).to_string();
                match rfd::FileDialog::new()
                    .add_filter("OpenRaster (layered, native)", &["ora"])
                    .add_filter("Photoshop (layered)", &["psd"])
                    .set_file_name(format!("{stem}.ora"))
                    .save_file()
                {
                    Some(p) => {
                        if p.extension().is_none() {
                            p.with_extension("ora")
                        } else {
                            p
                        }
                    }
                    None => return false,
                }
            }
        };
        match forge_io::save(&tab.doc.state, &path) {
            Ok(()) => {
                tab.doc.mark_saved();
                tab.doc.name = path.file_name().and_then(|n| n.to_str()).unwrap_or("Untitled").to_string();
                tab.doc.path = Some(path.clone());
                let _ = std::fs::remove_file(recovery_dir().join(format!("{}.ora", tab.id)));
                self.tools.status = "Saved".into();
                self.remember(&path);
                true
            }
            Err(e) => {
                self.error(format!("Could not save:\n{e}"));
                false
            }
        }
    }

    pub fn export(&mut self, quick: Option<&str>) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let stem = tab.doc.name.rsplit_once('.').map_or(tab.doc.name.as_str(), |(s, _)| s).to_string();
        let mut dlg = rfd::FileDialog::new();
        match quick {
            Some("pdf") => dlg = dlg.add_filter("PDF", &["pdf"]),
            Some(ext) => dlg = dlg.add_filter(ext.to_uppercase(), &[ext]),
            None => {
                for (n, e) in [
                    ("PNG", &["png"][..]),
                    ("JPEG", &["jpg", "jpeg"]),
                    ("WebP", &["webp"]),
                    ("TIFF", &["tif", "tiff"]),
                    ("PDF", &["pdf"]),
                    ("SVG (image + paths)", &["svg"]),
                    ("Photoshop", &["psd"]),
                    ("BMP", &["bmp"]),
                    ("TGA", &["tga"]),
                    ("GIF", &["gif"]),
                    ("QOI", &["qoi"]),
                    ("ICO", &["ico"]),
                ] {
                    dlg = dlg.add_filter(n, e);
                }
            }
        }
        let ext = quick.unwrap_or("png");
        let Some(mut path) = dlg.set_file_name(format!("{stem}.{ext}")).save_file() else { return };
        if path.extension().is_none() {
            path.set_extension(ext);
        }
        match forge_io::export(&tab.doc.state, &path, forge_io::ExportOptions::default()) {
            Ok(()) => self.tools.status = format!("Exported {}", path.display()),
            Err(e) => self.error(format!("Could not export:\n{e}")),
        }
    }

    pub fn export_layers(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let Some(dir) = rfd::FileDialog::new().pick_folder() else { return };
        let stem = tab.doc.name.rsplit_once('.').map_or(tab.doc.name.as_str(), |(s, _)| s).to_string();
        match forge_io::export_layers(&tab.doc.state, &dir, &stem) {
            Ok(n) => self.tools.status = format!("Exported {n} layers to {}", dir.display()),
            Err(e) => self.error(format!("Could not export layers:\n{e}")),
        }
    }

    pub fn place(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Images", forge_io::OPEN_EXTENSIONS).pick_file() else { return };
        let img = match forge_io::load_image(&path) {
            Ok(i) => i,
            Err(e) => return self.error(format!("Could not place {}:\n{e}", path.display())),
        };
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let (w, h) = (tab.doc.width(), tab.doc.height());
        let dx = (w as i32 - img.width() as i32) / 2;
        let dy = (h as i32 - img.height() as i32) / 2;
        let pixels = transform::translate(&img, dx, dy, w, h);
        let id = tab.doc.state.alloc_id();
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Placed").to_string();
        // Placed images are Smart Objects, like in Photoshop.
        let mut l = Layer::with_pixels(id, name, pixels.clone());
        l.smart = Some(Box::new(forge_core::layer::SmartObject::new(pixels)));
        tab.doc.add_layer(l, "Place");
    }

    pub fn request_close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs[i].doc.is_modified() {
            self.active = i;
            self.dialog = Some(Dialog::ConfirmClose { tab: i, quit: false });
        } else {
            self.close_tab(i);
        }
    }

    fn close_tab(&mut self, i: usize) {
        let t = self.tabs.remove(i);
        let _ = std::fs::remove_file(recovery_dir().join(format!("{}.ora", t.id)));
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
    }

    /// Writes modified documents to the recovery folder every two minutes.
    fn autosave(&mut self) {
        if self.last_autosave.elapsed() < Duration::from_secs(120) {
            return;
        }
        self.last_autosave = Instant::now();
        let dir = recovery_dir();
        let _ = std::fs::create_dir_all(&dir);
        for t in &self.tabs {
            let path = dir.join(format!("{}.ora", t.id));
            if t.doc.is_modified() {
                if let Ok(bytes) = forge_io::write_ora(&t.doc.state) {
                    let _ = std::fs::write(path, bytes);
                }
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    // ------------------------------------------------------------ clipboard

    pub fn copy(&mut self, merged: bool, cut: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let sel = tab.doc.state.selection.clone();
        let src = if merged {
            composite::merged(&tab.doc.state)
        } else {
            match tab.doc.state.active_layer() {
                Some(l) => l.pixels.clone(),
                None => return,
            }
        };
        let bounds = match &sel {
            Some(s) => s.content_bounds(),
            None => src.content_bounds(),
        };
        let Some(bounds) = bounds else { return };
        let mut pixels: TiledBuffer<Rgba8> = TiledBuffer::new(bounds.width() as u32, bounds.height() as u32);
        pixels.par_map_all(|x, y, _| {
            let (gx, gy) = (x + bounds.x0, y + bounds.y0);
            let mut p = src.get(gx, gy);
            p[3] = (p[3] as f32 * selection::coverage(sel.as_ref(), gx, gy) + 0.5) as u8;
            p
        });
        self.clipboard = Some(Clip { pixels, origin: bounds });
        if cut && !merged {
            self.clear_selection_pixels("Cut");
        }
    }

    pub fn paste(&mut self) {
        let Some(clip) = &self.clipboard else { return };
        let (cw, ch) = (clip.pixels.width() as i32, clip.pixels.height() as i32);
        let origin = clip.origin;
        let pixels_src = clip.pixels.clone();
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let (w, h) = (tab.doc.width(), tab.doc.height());
        let (dx, dy) = if origin.x1 <= w as i32 && origin.y1 <= h as i32 {
            (origin.x0, origin.y0)
        } else {
            ((w as i32 - cw) / 2, (h as i32 - ch) / 2)
        };
        let pixels = transform::translate(&pixels_src, dx, dy, w, h);
        let id = tab.doc.state.alloc_id();
        tab.doc.set_selection(None);
        tab.doc.add_layer(Layer::with_pixels(id, "Pasted", pixels), "Paste");
    }

    pub fn paste_as_new_document(&mut self) {
        let Some(clip) = &self.clipboard else { return };
        let mut doc = Document::new("Clipboard", clip.pixels.width(), clip.pixels.height(), None);
        doc.state.layers[0].pixels = clip.pixels.clone();
        doc.state.layers[0].name = "Layer 1".into();
        self.add_tab(doc);
    }

    pub fn clear_selection_pixels(&mut self, label: &str) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let sel = tab.doc.state.selection.clone();
        let editing_mask = tab.doc.state.editing_mask;
        let Some(layer) = tab.doc.state.active_layer_mut() else { return };
        if layer.locked {
            return;
        }
        let r = if editing_mask && layer.mask.is_some() {
            let m = layer.mask.as_mut().unwrap();
            let rect = selection::work_rect(sel.as_ref(), m.width(), m.height());
            m.par_for_each_in_rect_mut(rect, |x, y, v| {
                let s = selection::coverage(sel.as_ref(), x, y);
                *v = (*v as f32 * (1.0 - s)) as u8;
            });
            Some(rect)
        } else if layer.lock_alpha || !layer.has_pixels() {
            None
        } else {
            fill::clear(&mut layer.pixels, sel.as_ref())
        };
        if let Some(r) = r {
            tab.doc.mark_dirty(r);
            tab.doc.commit(label);
        }
    }

    // -------------------------------------------------------------- dialogs

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let outcome = match &mut dialog {
            Dialog::New(d) => d.show(ctx, &self.tools),
            Dialog::About => dialogs::about(ctx),
            Dialog::Shortcuts => dialogs::shortcuts(ctx),
            Dialog::Message(t, m) => dialogs::message(ctx, t, m),
            Dialog::ConfirmClose { tab, quit } => {
                let (tab, quit) = (*tab, *quit);
                self.confirm_close(ctx, tab, quit)
            }
            Dialog::Recovery(files) => {
                let files = files.clone();
                self.recovery(ctx, &files)
            }
            Dialog::Batch(d) => {
                let r = d.show(ctx, &self.recorder);
                if let Some((ai, input, output, ext)) = d.run.take() {
                    if let Some(action) = self.recorder.actions.get(ai).cloned() {
                        let (n, errors) = crate::actions::batch(self, &action, &input, &output, &ext, ctx);
                        d.result = Some(if errors.is_empty() {
                            format!("Processed {n} file(s).")
                        } else {
                            format!("Processed {n} file(s), {} error(s):\n{}", errors.len(), errors.join("\n"))
                        });
                    }
                }
                r
            }
            other => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return;
                };
                match other {
                    Dialog::ImageSize(d) => d.show(ctx, tab),
                    Dialog::CanvasSize(d) => d.show(ctx, tab),
                    Dialog::Adjust(d) => {
                        let r = d.show(ctx, tab, &self.tools);
                        if d.committed && self.recorder.recording {
                            self.recorder.push(crate::actions::ActionStep::Adjust(d.adj.clone()));
                        }
                        r
                    }
                    Dialog::Filter(d) => {
                        let r = d.show(ctx, tab, &self.tools, &mut self.last_filter);
                        if d.committed && self.recorder.recording {
                            self.recorder.push(crate::actions::ActionStep::Filter(d.filter.clone()));
                        }
                        r
                    }
                    Dialog::Transform(d) => d.show(ctx, tab),
                    Dialog::Text(d) => d.show(ctx, tab, &mut self.tools, &self.fonts),
                    Dialog::Fill(d) => d.show(ctx, tab, &self.tools),
                    Dialog::Stroke(d) => d.show(ctx, tab),
                    Dialog::SelModify(d) => d.show(ctx, tab),
                    Dialog::LayerStyle(d) => d.show(ctx, tab),
                    Dialog::ColorRange(d) => d.show(ctx, tab),
                    Dialog::FocusArea(d) => d.show(ctx, tab),
                    Dialog::NewGuide(d) => d.show(ctx, tab),
                    Dialog::Animation(d) => d.show(ctx, tab),
                    _ => Outcome::Close,
                }
            }
        };
        match outcome {
            Outcome::Open => {
                if self.dialog.is_none() {
                    self.dialog = Some(dialog);
                }
            }
            Outcome::Close => {}
            Outcome::NewDocument(doc) => self.add_tab(doc),
        }
    }

    fn recovery(&mut self, ctx: &egui::Context, files: &[PathBuf]) -> Outcome {
        let mut out = Outcome::Open;
        egui::Window::new("Recover Documents")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!("Forge Design found {} unsaved document(s) from a previous session.", files.len()));
                ui.horizontal(|ui| {
                    if ui.button("Recover").clicked() {
                        for f in files {
                            if let Ok(mut d) = forge_io::open(f) {
                                d.path = None;
                                d.name = "Recovered".into();
                                d.commit("Recovered");
                                self.add_tab(d);
                            }
                            let _ = std::fs::remove_file(f);
                        }
                        out = Outcome::Close;
                    }
                    if ui.button("Discard").clicked() {
                        for f in files {
                            let _ = std::fs::remove_file(f);
                        }
                        out = Outcome::Close;
                    }
                });
            });
        out
    }

    fn confirm_close(&mut self, ctx: &egui::Context, tab: usize, quit: bool) -> Outcome {
        let mut out = Outcome::Open;
        let name = self.tabs.get(tab).map(|t| t.doc.name.clone()).unwrap_or_default();
        egui::Window::new("Unsaved Changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!("Save changes to \"{name}\" before closing?"));
                ui.horizontal(|ui| {
                    let mut close = false;
                    if ui.button("Save").clicked() {
                        self.active = tab;
                        close = self.save(false);
                    }
                    if ui.button("Don't Save").clicked() {
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        out = Outcome::Close;
                    }
                    if close {
                        out = Outcome::Close;
                        if tab < self.tabs.len() {
                            self.close_tab(tab);
                        }
                        if quit {
                            self.continue_quit(ctx);
                        }
                    }
                });
            });
        out
    }

    fn continue_quit(&mut self, ctx: &egui::Context) {
        if let Some(i) = self.tabs.iter().position(|t| t.doc.is_modified()) {
            self.dialog = Some(Dialog::ConfirmClose { tab: i, quit: true });
        } else {
            self.quit_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    // -------------------------------------------------------------- panels

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut action = None;
        let mut cmd = None;
        let mut play = None;
        for g in 0..PANEL_GROUPS.len() {
            let group = PANEL_GROUPS[g];
            egui::Frame::none()
                .fill(Color32::from_rgb(30, 29, 35))
                .rounding(4.0)
                .inner_margin(egui::Margin::same(6.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    // Tab strip.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (k, p) in group.iter().enumerate() {
                            let sel = self.panels.tabs[g] == k;
                            let text = RichText::new(p.name()).size(12.0);
                            let text = if sel { text.color(ACCENT).strong() } else { text.color(Color32::from_gray(170)) };
                            if ui.selectable_label(sel, text).clicked() {
                                self.panels.tabs[g] = k;
                                self.panels.collapsed[g] = false;
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let c = self.panels.collapsed[g];
                            if ui.small_button(if c { "▼" } else { "▲" }).on_hover_text(if c { "Expand" } else { "Collapse" }).clicked() {
                                self.panels.collapsed[g] = !c;
                            }
                        });
                    });
                    if self.panels.collapsed[g] {
                        return;
                    }
                    ui.separator();
                    let id = group[self.panels.tabs[g].min(group.len() - 1)];
                    match id {
                        PanelId::Color => panels::color_panel(ui, &mut self.tools, self.editing_bg, &mut self.hex_buf),
                        PanelId::Swatches => panels::swatches_panel(ui, &mut self.tools, &mut self.swatches),
                        PanelId::Gradients => panels::gradients_panel(ui, &mut self.tools),
                        PanelId::Patterns => panels::patterns_panel(ui, &mut self.tools),
                        PanelId::Brushes => panels::brushes_panel(ui, &mut self.tools, &mut self.brush_presets),
                        PanelId::Actions => play = panels::actions_panel(ui, &mut self.recorder),
                        PanelId::Adjustments => cmd = panels::adjustments_panel(ui, !self.tabs.is_empty()),
                        other => {
                            let Some(tab) = self.tabs.get_mut(self.active) else {
                                ui.label(RichText::new("No document open").weak());
                                return;
                            };
                            match other {
                                PanelId::Properties => panels::properties_panel(ui, tab, &self.fonts),
                                PanelId::Info => panels::info_panel(ui, tab, &self.tools),
                                PanelId::Navigator => panels::navigator_panel(ui, tab),
                                PanelId::Histogram => panels::histogram_panel(ui, tab),
                                PanelId::Layers => action = panels::layers_panel(ui, tab, &mut self.layers_ui),
                                PanelId::Channels => cmd = panels::channels_panel(ui, tab),
                                PanelId::Paths => panels::paths_panel(ui, tab, &self.tools),
                                PanelId::History => {
                                    ui.set_max_height(180.0);
                                    panels::history_panel(ui, tab);
                                }
                                _ => {}
                            }
                        }
                    }
                });
            ui.add_space(4.0);
        }
        if let Some(i) = play {
            if let Some(a) = self.recorder.actions.get(i).cloned() {
                crate::actions::play(self, &a, &ctx);
            }
        }
        if let Some(c) = cmd {
            self.run(c, &ctx);
        }
        match action {
            Some(LayerAction::Cmd(c)) => self.run(c, &ctx),
            Some(LayerAction::SelectionFromAlpha(i)) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    crate::commands::selection_from_alpha(tab, i);
                }
            }
            Some(LayerAction::EditText(i)) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(d) = dialogs::TextDialog::edit(tab, i) {
                        self.dialog = Some(Dialog::Text(Box::new(d)));
                    }
                }
            }
            None => {}
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.recorder.recording {
                ui.label(RichText::new("● REC").color(Color32::from_rgb(240, 70, 70)));
                ui.separator();
            }
            if let Some(tab) = self.tabs.get(self.active) {
                ui.label(format!("{:.1}%", tab.view.zoom * 100.0));
                ui.separator();
                ui.label(format!("{} × {} px", tab.doc.width(), tab.doc.height()));
                ui.separator();
                if let Some(p) = tab.view.hover {
                    let (x, y) = (p.x.floor() as i32, p.y.floor() as i32);
                    if tab.doc.state.bounds().contains(x, y) {
                        ui.label(format!("X: {x}  Y: {y}"));
                        ui.separator();
                    }
                }
                let tiles: usize = tab
                    .doc
                    .state
                    .layers
                    .iter()
                    .map(|l| l.pixels.allocated_tiles() + l.mask.as_ref().map_or(0, |m| m.allocated_tiles() / 4))
                    .sum();
                ui.label(format!("Doc: {:.1} MB", tiles as f64 * 16.0 / 1024.0));
                if tab.doc.state.editing_mask {
                    ui.separator();
                    ui.label(RichText::new("Editing layer mask").color(Color32::from_rgb(255, 200, 80)));
                }
            }
            if !self.tools.status.is_empty() {
                ui.separator();
                ui.label(RichText::new(&self.tools.status).color(Color32::from_rgb(255, 210, 120)));
            }
        });
    }

    /// Start screen: "Projects" with New/Open and recent projects.
    fn welcome(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Frame::none().inner_margin(egui::Margin::same(32.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Projects").size(32.0).strong().color(PAPER));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_sized([110.0, 32.0], egui::Button::new("Open…")).clicked() {
                        self.run(Cmd::Open, &ctx);
                    }
                    if ui.add_sized([130.0, 32.0], egui::Button::new(RichText::new("+ New Project").strong()).fill(ACCENT_DIM)).clicked() {
                        self.run(Cmd::New, &ctx);
                    }
                });
            });
            ui.add_space(18.0);
            // Quick-start presets.
            ui.label(RichText::new("Start a new project").size(15.0).color(Color32::from_gray(200)));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for (name, w, h) in [
                    ("Full HD", 1920, 1080),
                    ("4K UHD", 3840, 2160),
                    ("A4 300 ppi", 2480, 3508),
                    ("Instagram Post", 1080, 1080),
                    ("Instagram Story", 1080, 1920),
                    ("YouTube Thumbnail", 1280, 720),
                    ("Logo / Icon", 1024, 1024),
                    ("Web Banner", 1840, 860),
                ] {
                    let label = RichText::new(format!("{name}\n{w} × {h}")).size(12.0);
                    if ui.add_sized([128.0, 56.0], egui::Button::new(label)).clicked() {
                        self.add_tab(Document::new(name, w, h, Some(Color::WHITE)));
                    }
                }
            });
            ui.add_space(22.0);
            ui.label(RichText::new("Recent projects").size(15.0).color(Color32::from_gray(200)));
            ui.add_space(6.0);
            if self.recent.is_empty() {
                ui.label(RichText::new("No recent projects yet. Open or drop images, PSD or ORA files here — you can also drop .ttf/.otf fonts to install them.").weak());
            }
            let mut open = None;
            let mut forget = None;
            egui::ScrollArea::vertical().max_height(ui.available_height() - 10.0).show(ui, |ui| {
                for (i, p) in self.recent.iter().enumerate() {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    let dir = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
                    ui.horizontal(|ui| {
                        let r = ui.add_sized([360.0, 34.0], egui::Button::new(RichText::new(name).size(14.0)).frame(true));
                        if r.clicked() {
                            open = Some(p.clone());
                        }
                        ui.label(RichText::new(dir).weak().size(11.0));
                        if ui.small_button("✖").on_hover_text("Remove from list").clicked() {
                            forget = Some(i);
                        }
                    });
                }
            });
            if let Some(i) = forget {
                self.recent.remove(i);
            }
            if let Some(p) = open {
                self.open_path(&p);
            }
        });
    }
}

impl eframe::App for ForgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Files dropped onto the window.
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(p) = f.path {
                self.open_path(&p);
            } else if let Some(bytes) = f.bytes {
                let ext = f.name.rsplit('.').next().unwrap_or("").to_string();
                if matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf") {
                    let tmp = std::env::temp_dir().join(&f.name);
                    if std::fs::write(&tmp, &bytes).is_ok() {
                        self.add_font(&tmp);
                    }
                    continue;
                }
                match forge_io::open_bytes(&bytes, &ext, &f.name) {
                    Ok(d) => self.add_tab(d),
                    Err(e) => self.error(format!("Could not open {}: {e}", f.name)),
                }
            }
        }

        // Ask before discarding unsaved work.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_confirmed {
            if self.tabs.iter().any(|t| t.doc.is_modified()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if self.dialog.is_none() {
                    self.continue_quit(ctx);
                }
            } else {
                let _ = std::fs::remove_dir_all(recovery_dir());
            }
        }

        self.handle_shortcuts(ctx);
        self.autosave();
        if self.updater.poll(ctx) {
            // The setup wizard is running; close so it can replace the files.
            self.quit_confirmed = !self.tabs.iter().any(|t| t.doc.is_modified());
            if self.quit_confirmed {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.tools.status = "Installer started — save your work and close Forge Design to finish the update".into();
            }
        }
        if std::mem::take(&mut self.tools.add_font_request) {
            self.add_font_dialog();
        }

        // Text tool clicked: open the type dialog (new or re-edit).
        if let Some((p, hit)) = self.tools.text_request.take() {
            if self.dialog.is_none() {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let d = match hit {
                        Some(i) => dialogs::TextDialog::edit(tab, i),
                        None => {
                            let mut data = self.tools.text.clone();
                            data.color = self.tools.fg;
                            data.vertical = self.tools.tool == crate::tools::Tool::VerticalText;
                            if let Some(f) = self.fonts.get(self.tools.font_index) {
                                data.font_name = f.name.clone();
                                data.font_path = f.path.as_ref().map(|p| p.to_string_lossy().to_string());
                            }
                            Some(dialogs::TextDialog::create(tab, p, data))
                        }
                    };
                    if let Some(d) = d {
                        self.dialog = Some(Dialog::Text(Box::new(d)));
                    }
                }
            }
        }

        // Window title.
        let title = match self.tabs.get(self.active) {
            Some(t) => format!("{} @ {:.0}% — Forge Design", t.title(), t.view.zoom * 100.0),
            None => "Forge Design".to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::top("options").show(ctx, |ui| {
            let tab = self.tabs.get_mut(self.active);
            self.tools.options_ui(ui, &self.fonts, tab);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        if !self.hide_panels {
            egui::SidePanel::left("tools").resizable(false).exact_width(48.0).show(ctx, |ui| {
                ui.add_space(4.0);
                egui::ScrollArea::vertical().show(ui, |ui| panels::toolbar(ui, &mut self.tools, &mut self.editing_bg));
            });
            egui::SidePanel::right("panels").default_width(300.0).min_width(250.0).show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.right_panel(ui));
            });
        }

        egui::CentralPanel::default().frame(egui::Frame::none().fill(INK)).show(ctx, |ui| {
            if self.tabs.is_empty() {
                self.welcome(ui);
                return;
            }
            // Document tabs.
            let mut close = None;
            egui::Frame::none().fill(Color32::from_rgb(34, 33, 39)).inner_margin(egui::Margin::symmetric(4.0, 2.0)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, t) in self.tabs.iter().enumerate() {
                        let selected = i == self.active;
                        if ui.selectable_label(selected, t.title()).clicked() && !self.tools.is_busy() && self.tools.transform.is_none() {
                            self.active = i;
                        }
                        if ui.small_button("×").on_hover_text("Close").clicked() {
                            close = Some(i);
                        }
                        ui.separator();
                    }
                });
            });
            if let Some(i) = close {
                self.request_close_tab(i);
            }
            if let Some(tab) = self.tabs.get_mut(self.active) {
                canvas::canvas_ui(ui, tab, &mut self.tools, self.dialog.is_none());
            }
        });

        self.dialogs(ctx);
        self.updater.ui(ctx);
    }
}

pub fn setup_style(ctx: &egui::Context, light: bool) {
    let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    if light {
        v.selection.bg_fill = Color32::from_rgb(170, 210, 60);
        v.selection.stroke = egui::Stroke::new(1.0_f32, Color32::BLACK);
        v.hyperlink_color = Color32::from_rgb(70, 110, 10);
    } else {
        v.panel_fill = Color32::from_rgb(34, 33, 39);
        v.window_fill = Color32::from_rgb(38, 37, 44);
        v.extreme_bg_color = INK;
        v.faint_bg_color = Color32::from_rgb(44, 43, 50);
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
        v.hyperlink_color = ACCENT;
        v.widgets.noninteractive.bg_fill = Color32::from_rgb(34, 33, 39);
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, PAPER.gamma_multiply(0.8));
        v.widgets.inactive.bg_fill = Color32::from_rgb(52, 51, 59);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(48, 47, 55);
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, PAPER);
        v.widgets.hovered.bg_fill = Color32::from_rgb(66, 65, 74);
        v.widgets.active.bg_fill = ACCENT_DIM;
    }
    v.window_rounding = egui::Rounding::same(4.0);
    ctx.set_visuals(v);
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(6.0, 3.0);
    ctx.set_style(style);
}
