//! The application shell: menus, shortcuts, panels, tabs and dialogs.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers, RichText};
use forge_core::adjust::{self, Adjustment, Channel, Levels};
use forge_core::buffer::TiledBuffer;
use forge_core::document::Document;
use forge_core::filters::Filter;
use forge_core::selection::{self, Mask};
use forge_core::transform::{self, Orientation};
use forge_core::{composite, fill, BlendMode, Color, IRect, Layer, Rgba8};

use crate::canvas::{self, DocTab};
use crate::dialogs::{self, Outcome};
use crate::panels::{self, LayerAction, LayersUi};
use crate::text::{self, FontEntry};
use crate::tools::{Tool, ToolState};

enum Dialog {
    New(dialogs::NewDoc),
    ImageSize(dialogs::ImageSize),
    CanvasSize(dialogs::CanvasSize),
    Adjust(Box<dialogs::AdjustDialog>),
    Filter(Box<dialogs::FilterDialog>),
    Transform(Box<dialogs::TransformDialog>),
    Text(dialogs::TextDialog),
    Fill(dialogs::FillDialog),
    Stroke(dialogs::StrokeDialog),
    SelModify(dialogs::SelModify),
    About,
    Shortcuts,
    Message(String, String),
    ConfirmClose { tab: usize, quit: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    New,
    Open,
    Place,
    Save,
    SaveAs,
    Export,
    Close,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    CopyMerged,
    Paste,
    Clear,
    FillDialog,
    FillFg,
    FillBg,
    Stroke,
    Transform,
    ImageSize,
    CanvasSize,
    Orient(Orientation),
    CropToSelection,
    Trim,
    Flatten,
    AutoContrast,
    Adjust(usize),
    Filter(usize),
    RepeatFilter,
    NewLayer,
    DuplicateOrViaCopy,
    DeleteLayer,
    MergeDown,
    StampVisible,
    LayerUp,
    LayerDown,
    LayerTop,
    LayerBottom,
    AddMask,
    MaskFromSelection,
    DeleteMask,
    ApplyMask,
    ToggleLockAlpha,
    SelectAll,
    Deselect,
    Inverse,
    SelModify(dialogs::SelModifyKind),
    SelectionFromAlpha,
    ZoomIn,
    ZoomOut,
    Fit,
    ActualPixels,
    About,
    Shortcuts,
}

/// Adjustment menu entries: (label, shortcut).
const ADJUSTMENTS: [(&str, &str); 14] = [
    ("Brightness/Contrast…", ""),
    ("Levels…", "Ctrl+L"),
    ("Curves…", "Ctrl+M"),
    ("Exposure…", ""),
    ("Vibrance…", ""),
    ("Hue/Saturation…", "Ctrl+U"),
    ("Color Balance…", "Ctrl+B"),
    ("Black & White", "Alt+Shift+Ctrl+B"),
    ("Photo Filter…", ""),
    ("Gradient Map…", ""),
    ("Invert", "Ctrl+I"),
    ("Posterize…", ""),
    ("Threshold…", ""),
    ("Desaturate", "Shift+Ctrl+U"),
];

fn default_adjustment(i: usize, fg: Color, bg: Color) -> Adjustment {
    match i {
        0 => Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.0 },
        1 => Adjustment::Levels { channel: Channel::Rgb, levels: Levels::default() },
        2 => Adjustment::Curves { channel: Channel::Rgb, points: vec![(0.0, 0.0), (255.0, 255.0)] },
        3 => Adjustment::Exposure { exposure: 0.0, offset: 0.0, gamma: 1.0 },
        4 => Adjustment::Vibrance { vibrance: 0.0, saturation: 0.0 },
        5 => Adjustment::HueSaturation { hue: 0.0, saturation: 0.0, lightness: 0.0, colorize: false },
        6 => Adjustment::ColorBalance { shadows: [0.0; 3], midtones: [0.0; 3], highlights: [0.0; 3] },
        7 => Adjustment::BlackAndWhite,
        8 => Adjustment::PhotoFilter { color: Color::rgb(0.93, 0.54, 0.1), density: 0.25 },
        9 => Adjustment::GradientMap { from: fg, to: bg },
        10 => Adjustment::Invert,
        11 => Adjustment::Posterize { levels: 4 },
        12 => Adjustment::Threshold { level: 128 },
        _ => Adjustment::Desaturate,
    }
}

/// Filter menu: (submenu, label, factory).
const FILTERS: [(&str, &str); 14] = [
    ("Blur", "Gaussian Blur…"),
    ("Blur", "Box Blur…"),
    ("Blur", "Motion Blur…"),
    ("Sharpen", "Sharpen"),
    ("Sharpen", "Unsharp Mask…"),
    ("Noise", "Add Noise…"),
    ("Noise", "Median…"),
    ("Distort", "Twirl…"),
    ("Distort", "Wave…"),
    ("Pixelate", "Mosaic…"),
    ("Stylize", "Emboss…"),
    ("Stylize", "Find Edges"),
    ("Other", "High Pass…"),
    ("Render", "Clouds…"),
];

fn default_filter(i: usize, fg: Color, bg: Color) -> Filter {
    match i {
        0 => Filter::GaussianBlur { radius: 4.0 },
        1 => Filter::BoxBlur { radius: 4 },
        2 => Filter::MotionBlur { angle: 0.0, distance: 20.0 },
        3 => Filter::Sharpen,
        4 => Filter::UnsharpMask { amount: 1.0, radius: 1.5, threshold: 0 },
        5 => Filter::AddNoise { amount: 0.12, monochrome: false, seed: 1 },
        6 => Filter::Median { radius: 2 },
        7 => Filter::Twirl { angle: 120.0 },
        8 => Filter::Wave { amplitude: 10.0, wavelength: 120.0 },
        9 => Filter::Pixelate { size: 12 },
        10 => Filter::Emboss { angle: 135.0, amount: 1.0 },
        11 => Filter::FindEdges,
        12 => Filter::HighPass { radius: 10.0 },
        _ => Filter::Clouds { fg, bg, scale: 200.0, seed: 7 },
    }
}

fn instant_filter(f: &Filter) -> bool {
    matches!(f, Filter::Sharpen | Filter::FindEdges)
}

fn instant_adjustment(a: &Adjustment) -> bool {
    matches!(a, Adjustment::Invert | Adjustment::Desaturate | Adjustment::BlackAndWhite)
}

/// Brand colours (from the logo).
pub const ACCENT: Color32 = Color32::from_rgb(198, 244, 50);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(78, 96, 30);
pub const INK: Color32 = Color32::from_rgb(24, 23, 28);
pub const PAPER: Color32 = Color32::from_rgb(238, 234, 226);

struct Clip {
    pixels: TiledBuffer<Rgba8>,
    origin: IRect,
}

pub struct ForgeApp {
    tabs: Vec<DocTab>,
    active: usize,
    tools: ToolState,
    dialog: Option<Dialog>,
    layers_ui: LayersUi,
    editing_bg: bool,
    hex_buf: String,
    fonts: Vec<FontEntry>,
    clipboard: Option<Clip>,
    last_filter: Option<Filter>,
    show_color: bool,
    show_histogram: bool,
    show_layers: bool,
    show_history: bool,
    quit_confirmed: bool,
    title: String,
}

impl ForgeApp {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        setup_style(&cc.egui_ctx);
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
            show_color: true,
            show_histogram: true,
            show_layers: true,
            show_history: true,
            quit_confirmed: false,
            title: String::new(),
        };
        for f in files {
            app.open_path(&f);
        }
        app
    }

    fn tab(&mut self) -> Option<&mut DocTab> {
        self.tabs.get_mut(self.active)
    }

    fn add_tab(&mut self, doc: Document) {
        self.tabs.push(DocTab::new(doc));
        self.active = self.tabs.len() - 1;
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.dialog = Some(Dialog::Message("Error".into(), msg.into()));
    }

    fn open_path(&mut self, path: &Path) {
        match forge_io::open(path) {
            Ok(doc) => self.add_tab(doc),
            Err(e) => self.error(format!("Could not open {}:\n{e}", path.display())),
        }
    }

    // ------------------------------------------------------------ file ops

    fn open_dialog(&mut self) {
        let files = rfd::FileDialog::new()
            .add_filter("All supported images", forge_io::OPEN_EXTENSIONS)
            .add_filter("OpenRaster", &["ora"])
            .add_filter("Photoshop", &["psd"])
            .pick_files();
        for f in files.unwrap_or_default() {
            self.open_path(&f);
        }
    }

    fn save(&mut self, save_as: bool) -> bool {
        let Some(tab) = self.tabs.get_mut(self.active) else { return false };
        let path = match (&tab.doc.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let stem = tab.doc.name.rsplit_once('.').map_or(tab.doc.name.as_str(), |(s, _)| s).to_string();
                match rfd::FileDialog::new().add_filter("OpenRaster (layered)", &["ora"]).set_file_name(format!("{stem}.ora")).save_file() {
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
                tab.doc.path = Some(path);
                self.tools.status = "Saved".into();
                true
            }
            Err(e) => {
                self.error(format!("Could not save:\n{e}"));
                false
            }
        }
    }

    fn export(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let stem = tab.doc.name.rsplit_once('.').map_or(tab.doc.name.as_str(), |(s, _)| s).to_string();
        let Some(mut path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"])
            .add_filter("WebP", &["webp"])
            .add_filter("TIFF", &["tif", "tiff"])
            .add_filter("BMP", &["bmp"])
            .add_filter("TGA", &["tga"])
            .add_filter("GIF", &["gif"])
            .add_filter("QOI", &["qoi"])
            .set_file_name(format!("{stem}.png"))
            .save_file()
        else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("png");
        }
        match forge_io::export(&tab.doc.state, &path, forge_io::ExportOptions::default()) {
            Ok(()) => self.tools.status = format!("Exported {}", path.display()),
            Err(e) => self.error(format!("Could not export:\n{e}")),
        }
    }

    fn place(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Images", forge_io::OPEN_EXTENSIONS).pick_file() else { return };
        let img = match forge_io::load_image(&path) {
            Ok(i) => i,
            Err(e) => return self.error(format!("Could not place {}:\n{e}", path.display())),
        };
        let Some(tab) = self.tab() else { return };
        let (w, h) = (tab.doc.width(), tab.doc.height());
        let dx = (w as i32 - img.width() as i32) / 2;
        let dy = (h as i32 - img.height() as i32) / 2;
        let pixels = transform::translate(&img, dx, dy, w, h);
        let id = tab.doc.state.alloc_id();
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Placed").to_string();
        tab.doc.add_layer(Layer::with_pixels(id, name, pixels), "Place");
    }

    fn request_close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs[i].doc.is_modified() {
            self.active = i;
            self.dialog = Some(Dialog::ConfirmClose { tab: i, quit: false });
        } else {
            self.tabs.remove(i);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        }
    }

    // ------------------------------------------------------------ clipboard

    fn copy(&mut self, merged: bool, cut: bool) {
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

    fn paste(&mut self) {
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

    fn paste_as_new_document(&mut self) {
        let Some(clip) = &self.clipboard else { return };
        let mut doc = Document::new("Clipboard", clip.pixels.width(), clip.pixels.height(), None);
        doc.state.layers[0].pixels = clip.pixels.clone();
        doc.state.layers[0].name = "Layer 1".into();
        self.add_tab(doc);
    }

    fn clear_selection_pixels(&mut self, label: &str) {
        let Some(tab) = self.tab() else { return };
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
        } else if layer.lock_alpha {
            None
        } else {
            fill::clear(&mut layer.pixels, sel.as_ref())
        };
        if let Some(r) = r {
            tab.doc.mark_dirty(r);
            tab.doc.commit(label);
        }
    }

    // ------------------------------------------------------------- commands

    pub fn run(&mut self, cmd: Cmd, ctx: &egui::Context) {
        if self.tools.is_busy() {
            return;
        }
        let (fg, bg) = (self.tools.fg, self.tools.bg);
        match cmd {
            Cmd::New => {
                let mut nd = dialogs::NewDoc::default();
                if let Some(c) = &self.clipboard {
                    nd.width = c.pixels.width();
                    nd.height = c.pixels.height();
                }
                nd.name = format!("Untitled-{}", self.tabs.len() + 1);
                self.dialog = Some(Dialog::New(nd));
            }
            Cmd::Open => self.open_dialog(),
            Cmd::Place => self.place(),
            Cmd::Save => {
                self.save(false);
            }
            Cmd::SaveAs => {
                self.save(true);
            }
            Cmd::Export => self.export(),
            Cmd::Close => self.request_close_tab(self.active),
            Cmd::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Cmd::About => self.dialog = Some(Dialog::About),
            Cmd::Shortcuts => self.dialog = Some(Dialog::Shortcuts),
            Cmd::Copy => self.copy(false, false),
            Cmd::CopyMerged => self.copy(true, false),
            Cmd::Cut => self.copy(false, true),
            Cmd::Paste => {
                if self.tabs.is_empty() {
                    self.paste_as_new_document();
                } else {
                    self.paste();
                }
            }
            Cmd::Clear => self.clear_selection_pixels("Clear"),
            _ => {}
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let (w, h) = (tab.doc.width(), tab.doc.height());
        let has_layer = tab.doc.state.active_layer().is_some();
        match cmd {
            Cmd::Undo => {
                tab.doc.undo();
            }
            Cmd::Redo => {
                tab.doc.redo();
            }
            Cmd::FillDialog => self.dialog = Some(Dialog::Fill(dialogs::FillDialog::new(&self.tools))),
            Cmd::FillFg => dialogs::fill_selection(tab, fg, BlendMode::Normal, 1.0, false, "Fill"),
            Cmd::FillBg => dialogs::fill_selection(tab, bg, BlendMode::Normal, 1.0, false, "Fill"),
            Cmd::Stroke => {
                if tab.doc.selection().is_some() {
                    self.dialog = Some(Dialog::Stroke(dialogs::StrokeDialog::new(&self.tools)));
                }
            }
            Cmd::Transform => {
                if let Some(d) = dialogs::TransformDialog::new(tab) {
                    self.dialog = Some(Dialog::Transform(Box::new(d)));
                }
            }
            Cmd::ImageSize => self.dialog = Some(Dialog::ImageSize(dialogs::ImageSize::new(tab))),
            Cmd::CanvasSize => self.dialog = Some(Dialog::CanvasSize(dialogs::CanvasSize::new(tab))),
            Cmd::Orient(o) => {
                transform::reorient_document(&mut tab.doc.state, o);
                tab.doc.mark_all_dirty();
                tab.doc.set_selection(tab.doc.state.selection.clone());
                tab.doc.commit(o.name());
                tab.view.request_fit();
            }
            Cmd::CropToSelection => {
                if let Some(r) = tab.doc.selection().and_then(|s| s.content_bounds()) {
                    transform::crop(&mut tab.doc.state, r);
                    tab.doc.state.selection = None;
                    tab.doc.set_selection(None);
                    tab.doc.mark_all_dirty();
                    tab.doc.commit("Crop");
                    tab.view.request_fit();
                }
            }
            Cmd::Trim => {
                let merged = composite::merged(&tab.doc.state);
                if let Some(r) = merged.content_bounds() {
                    if r != tab.doc.state.bounds() {
                        transform::crop(&mut tab.doc.state, r);
                        tab.doc.set_selection(tab.doc.state.selection.clone());
                        tab.doc.mark_all_dirty();
                        tab.doc.commit("Trim");
                        tab.view.request_fit();
                    }
                }
            }
            Cmd::Flatten => tab.doc.flatten(Color::WHITE),
            Cmd::AutoContrast if has_layer => {
                let layer = tab.doc.state.active;
                let base = tab.doc.state.layers[layer].pixels.clone();
                let sel = tab.doc.state.selection.clone();
                let rect = selection::work_rect(sel.as_ref(), w, h);
                for adj in adjust::auto_levels(&base, rect) {
                    let b = tab.doc.state.layers[layer].pixels.clone();
                    adjust::apply(&mut tab.doc.state.layers[layer].pixels, &b, sel.as_ref(), &adj);
                }
                tab.doc.mark_dirty(rect);
                tab.doc.commit("Auto Contrast");
            }
            Cmd::Adjust(i) if has_layer => {
                let adj = default_adjustment(i, fg, bg);
                if instant_adjustment(&adj) {
                    let layer = tab.doc.state.active;
                    let base = tab.doc.state.layers[layer].pixels.clone();
                    let sel = tab.doc.state.selection.clone();
                    if let Some(r) = adjust::apply(&mut tab.doc.state.layers[layer].pixels, &base, sel.as_ref(), &adj) {
                        tab.doc.mark_dirty(r);
                        tab.doc.commit(adj.name());
                    }
                } else if let Some(d) = dialogs::AdjustDialog::new(tab, adj) {
                    self.dialog = Some(Dialog::Adjust(Box::new(d)));
                }
            }
            Cmd::Filter(i) if has_layer => {
                let f = default_filter(i, fg, bg);
                if instant_filter(&f) {
                    apply_filter_now(tab, &f);
                    self.last_filter = Some(f);
                } else if let Some(d) = dialogs::FilterDialog::new(tab, f) {
                    self.dialog = Some(Dialog::Filter(Box::new(d)));
                }
            }
            Cmd::RepeatFilter if has_layer => {
                if let Some(f) = self.last_filter.clone() {
                    apply_filter_now(tab, &f);
                }
            }
            Cmd::NewLayer => tab.doc.new_layer(),
            Cmd::DuplicateOrViaCopy => {
                if let (Some(sel), Some(l)) = (tab.doc.state.selection.clone(), tab.doc.state.active_layer()) {
                    let mut px = l.pixels.clone();
                    let name = format!("{} (copy)", l.name);
                    px.par_map_all(|x, y, mut p| {
                        p[3] = (p[3] as f32 * selection::coverage(Some(&sel), x, y) + 0.5) as u8;
                        p
                    });
                    px.optimize();
                    let id = tab.doc.state.alloc_id();
                    tab.doc.add_layer(Layer::with_pixels(id, name, px), "Layer via Copy");
                } else {
                    tab.doc.duplicate_layer();
                }
            }
            Cmd::DeleteLayer => tab.doc.delete_layer(),
            Cmd::MergeDown => tab.doc.merge_down(),
            Cmd::StampVisible => tab.doc.stamp_visible(),
            Cmd::LayerUp => tab.doc.move_layer(1),
            Cmd::LayerDown => tab.doc.move_layer(-1),
            Cmd::LayerTop => tab.doc.move_layer(i32::MAX / 2),
            Cmd::LayerBottom => tab.doc.move_layer(-i32::MAX / 2),
            Cmd::AddMask => tab.doc.add_layer_mask(false),
            Cmd::MaskFromSelection => tab.doc.add_layer_mask(true),
            Cmd::DeleteMask => tab.doc.delete_layer_mask(false),
            Cmd::ApplyMask => tab.doc.delete_layer_mask(true),
            Cmd::ToggleLockAlpha => {
                if let Some(l) = tab.doc.state.active_layer_mut() {
                    l.lock_alpha = !l.lock_alpha;
                    tab.doc.commit("Lock Change");
                }
            }
            Cmd::SelectAll => {
                tab.doc.set_selection(Some(selection::select_all(w, h)));
                tab.doc.commit("Select All");
            }
            Cmd::Deselect => {
                if tab.doc.selection().is_some() {
                    tab.doc.set_selection(None);
                    tab.doc.commit("Deselect");
                }
            }
            Cmd::Inverse => {
                let inv = selection::invert(tab.doc.selection(), w, h);
                if tab.doc.selection().is_some() {
                    tab.doc.set_selection(inv);
                    tab.doc.commit("Inverse");
                }
            }
            Cmd::SelModify(kind) => {
                if tab.doc.selection().is_some() {
                    let amount = if kind == dialogs::SelModifyKind::Feather { 5.0 } else { 3.0 };
                    self.dialog = Some(Dialog::SelModify(dialogs::SelModify { kind, amount }));
                }
            }
            Cmd::SelectionFromAlpha => {
                let i = tab.doc.state.active;
                selection_from_alpha(tab, i);
            }
            Cmd::ZoomIn => tab.view.zoom_step(1, None, w, h),
            Cmd::ZoomOut => tab.view.zoom_step(-1, None, w, h),
            Cmd::Fit => tab.view.request_fit(),
            Cmd::ActualPixels => {
                let c = tab.view.canvas_rect.center();
                tab.view.zoom_to(1.0, c, w, h);
            }
            _ => {}
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        use Key::*;
        let cmd = Modifiers::COMMAND;
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        let ca = Modifiers::COMMAND | Modifiers::ALT;
        let cas = Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT;
        // Most specific modifier combinations first: egui matches "logically".
        let table: [(Modifiers, Key, Cmd); 46] = [
            (cas, B, Cmd::Adjust(7)),
            (cas, E, Cmd::StampVisible),
            (cs, Z, Cmd::Redo),
            (cs, S, Cmd::SaveAs),
            (cs, E, Cmd::Export),
            (cs, N, Cmd::NewLayer),
            (cs, I, Cmd::Inverse),
            (cs, C, Cmd::CopyMerged),
            (cs, U, Cmd::Adjust(13)),
            (cs, L, Cmd::AutoContrast),
            (cs, CloseBracket, Cmd::LayerTop),
            (cs, OpenBracket, Cmd::LayerBottom),
            (ca, I, Cmd::ImageSize),
            (ca, C, Cmd::CanvasSize),
            (cmd, Z, Cmd::Undo),
            (cmd, Y, Cmd::Redo),
            (cmd, N, Cmd::New),
            (cmd, O, Cmd::Open),
            (cmd, S, Cmd::Save),
            (cmd, W, Cmd::Close),
            (cmd, Q, Cmd::Quit),
            (cmd, A, Cmd::SelectAll),
            (cmd, D, Cmd::Deselect),
            (cmd, X, Cmd::Cut),
            (cmd, C, Cmd::Copy),
            (cmd, V, Cmd::Paste),
            (cmd, J, Cmd::DuplicateOrViaCopy),
            (cmd, E, Cmd::MergeDown),
            (cmd, T, Cmd::Transform),
            (cmd, L, Cmd::Adjust(1)),
            (cmd, M, Cmd::Adjust(2)),
            (cmd, U, Cmd::Adjust(5)),
            (cmd, B, Cmd::Adjust(6)),
            (cmd, I, Cmd::Adjust(10)),
            (cmd, F, Cmd::RepeatFilter),
            (cmd, CloseBracket, Cmd::LayerUp),
            (cmd, OpenBracket, Cmd::LayerDown),
            (cmd, Plus, Cmd::ZoomIn),
            (cmd, Equals, Cmd::ZoomIn),
            (cmd, Minus, Cmd::ZoomOut),
            (cmd, Num0, Cmd::Fit),
            (cmd, Num1, Cmd::ActualPixels),
            (cmd, Backspace, Cmd::FillBg),
            (Modifiers::ALT, Backspace, Cmd::FillFg),
            (Modifiers::NONE, Delete, Cmd::Clear),
            (cs, Slash, Cmd::Shortcuts),
        ];
        let wants_kb = ctx.wants_keyboard_input();
        if wants_kb {
            return;
        }
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            for (m, k, c) in table {
                if i.consume_shortcut(&KeyboardShortcut::new(m, k)) {
                    fired.push(c);
                }
            }
        });
        for c in fired {
            self.run(c, ctx);
        }

        // Single-key tool shortcuts and brush tweaks.
        let (pressed, mods): (Vec<Key>, Modifiers) = ctx.input(|i| {
            (
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key { key, pressed: true, repeat: _, .. } => Some(*key),
                        _ => None,
                    })
                    .collect(),
                i.modifiers,
            )
        });
        if mods.command {
            return;
        }
        for key in pressed {
            if let Some(t) = Tool::ALL.iter().find(|t| t.shortcut() == Some(key)) {
                if self.tools.is_busy() {
                    continue;
                }
                if *t == Tool::Marquee && mods.shift && self.tools.tool.is_selection() {
                    self.tools.tool = if self.tools.tool == Tool::Marquee { Tool::EllipseMarquee } else { Tool::Marquee };
                } else if !(*t == Tool::Marquee && self.tools.tool == Tool::EllipseMarquee) {
                    self.tools.tool = *t;
                }
                continue;
            }
            match key {
                Key::X => self.tools.swap_colors(),
                Key::D => self.tools.reset_colors(),
                Key::OpenBracket | Key::CloseBracket => {
                    let up = key == Key::CloseBracket;
                    if let Some(b) = self.tools.current_brush() {
                        if mods.shift {
                            b.hardness = (b.hardness + if up { 0.25 } else { -0.25 }).clamp(0.0, 1.0);
                        } else {
                            let step = (b.size * 0.15).max(1.0);
                            b.size = (b.size + if up { step } else { -step }).clamp(1.0, 2000.0).round();
                        }
                    }
                }
                Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9 | Key::Num0 => {
                    let n = match key {
                        Key::Num0 => 10,
                        k => k.name().parse::<u32>().unwrap_or(10),
                    };
                    if let Some(b) = self.tools.current_brush() {
                        b.opacity = n as f32 / 10.0;
                    } else if matches!(self.tools.tool, Tool::Bucket | Tool::Gradient) {
                        self.tools.fill_opacity = n as f32 / 10.0;
                    }
                }
                Key::Backspace => {
                    if mods.alt {
                        self.run(Cmd::FillFg, ctx);
                    } else {
                        self.run(Cmd::Clear, ctx);
                    }
                }
                Key::Slash => self.run(Cmd::ToggleLockAlpha, ctx),
                Key::Enter => {
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        self.tools.apply_crop(tab);
                    }
                }
                Key::Escape => {
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        self.tools.cancel(tab);
                    }
                }
                Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown if self.tools.tool == Tool::Move => {
                    let n = if mods.shift { 10 } else { 1 };
                    let (dx, dy) = match key {
                        Key::ArrowLeft => (-n, 0),
                        Key::ArrowRight => (n, 0),
                        Key::ArrowUp => (0, -n),
                        _ => (0, n),
                    };
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        self.tools.nudge(tab, dx, dy);
                    }
                }
                _ => {}
            }
        }
    }

    // ---------------------------------------------------------------- menus

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let mut cmd: Option<Cmd> = None;
        let has_doc = !self.tabs.is_empty();
        let has_sel = self.tabs.get(self.active).is_some_and(|t| t.doc.selection().is_some());
        let (can_undo, can_redo) = self
            .tabs
            .get(self.active)
            .map_or((false, false), |t| (t.doc.history().can_undo(), t.doc.history().can_redo()));
        let mut item = |ui: &mut egui::Ui, label: &str, sc: &str, c: Cmd, enabled: bool| {
            if ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(sc)).clicked() {
                cmd = Some(c);
                ui.close_menu();
            }
        };
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                item(ui, "New…", "Ctrl+N", Cmd::New, true);
                item(ui, "Open…", "Ctrl+O", Cmd::Open, true);
                item(ui, "Place Embedded…", "", Cmd::Place, has_doc);
                ui.separator();
                item(ui, "Close", "Ctrl+W", Cmd::Close, has_doc);
                item(ui, "Save", "Ctrl+S", Cmd::Save, has_doc);
                item(ui, "Save As…", "Shift+Ctrl+S", Cmd::SaveAs, has_doc);
                item(ui, "Export As…", "Shift+Ctrl+E", Cmd::Export, has_doc);
                ui.separator();
                item(ui, "Exit", "Ctrl+Q", Cmd::Quit, true);
            });
            ui.menu_button("Edit", |ui| {
                let undo_label = self.tabs.get(self.active).filter(|_| can_undo).map(|t| format!("Undo {}", t.doc.history().current().label)).unwrap_or("Undo".into());
                item(ui, &undo_label, "Ctrl+Z", Cmd::Undo, can_undo);
                item(ui, "Redo", "Shift+Ctrl+Z", Cmd::Redo, can_redo);
                ui.separator();
                item(ui, "Cut", "Ctrl+X", Cmd::Cut, has_doc);
                item(ui, "Copy", "Ctrl+C", Cmd::Copy, has_doc);
                item(ui, "Copy Merged", "Shift+Ctrl+C", Cmd::CopyMerged, has_doc);
                item(ui, "Paste", "Ctrl+V", Cmd::Paste, self.clipboard.is_some());
                item(ui, "Clear", "Del", Cmd::Clear, has_doc);
                ui.separator();
                item(ui, "Fill…", "", Cmd::FillDialog, has_doc);
                item(ui, "Stroke…", "", Cmd::Stroke, has_sel);
                ui.separator();
                item(ui, "Free Transform…", "Ctrl+T", Cmd::Transform, has_doc);
            });
            ui.menu_button("Image", |ui| {
                ui.menu_button("Adjustments", |ui| {
                    for (i, (label, sc)) in ADJUSTMENTS.iter().enumerate() {
                        if i == 3 || i == 7 || i == 10 || i == 13 {
                            ui.separator();
                        }
                        item(ui, label, sc, Cmd::Adjust(i), has_doc);
                    }
                });
                item(ui, "Auto Contrast", "Shift+Ctrl+L", Cmd::AutoContrast, has_doc);
                ui.separator();
                item(ui, "Image Size…", "Alt+Ctrl+I", Cmd::ImageSize, has_doc);
                item(ui, "Canvas Size…", "Alt+Ctrl+C", Cmd::CanvasSize, has_doc);
                ui.menu_button("Image Rotation", |ui| {
                    for o in [Orientation::Rotate180, Orientation::Rotate90Cw, Orientation::Rotate90Ccw, Orientation::FlipHorizontal, Orientation::FlipVertical] {
                        item(ui, o.name(), "", Cmd::Orient(o), has_doc);
                    }
                });
                item(ui, "Crop to Selection", "", Cmd::CropToSelection, has_sel);
                item(ui, "Trim Transparent", "", Cmd::Trim, has_doc);
                ui.separator();
                item(ui, "Flatten Image", "", Cmd::Flatten, has_doc);
            });
            ui.menu_button("Layer", |ui| {
                item(ui, "New Layer", "Shift+Ctrl+N", Cmd::NewLayer, has_doc);
                item(ui, if has_sel { "Layer via Copy" } else { "Duplicate Layer" }, "Ctrl+J", Cmd::DuplicateOrViaCopy, has_doc);
                item(ui, "Delete Layer", "", Cmd::DeleteLayer, has_doc);
                ui.separator();
                ui.menu_button("Layer Mask", |ui| {
                    item(ui, "Reveal All", "", Cmd::AddMask, has_doc);
                    item(ui, "Reveal Selection", "", Cmd::MaskFromSelection, has_sel);
                    item(ui, "Delete", "", Cmd::DeleteMask, has_doc);
                    item(ui, "Apply", "", Cmd::ApplyMask, has_doc);
                });
                ui.menu_button("Arrange", |ui| {
                    item(ui, "Bring to Front", "Shift+Ctrl+]", Cmd::LayerTop, has_doc);
                    item(ui, "Bring Forward", "Ctrl+]", Cmd::LayerUp, has_doc);
                    item(ui, "Send Backward", "Ctrl+[", Cmd::LayerDown, has_doc);
                    item(ui, "Send to Back", "Shift+Ctrl+[", Cmd::LayerBottom, has_doc);
                });
                item(ui, "Lock Transparent Pixels", "/", Cmd::ToggleLockAlpha, has_doc);
                ui.separator();
                item(ui, "Merge Down", "Ctrl+E", Cmd::MergeDown, has_doc);
                item(ui, "Stamp Visible", "Alt+Shift+Ctrl+E", Cmd::StampVisible, has_doc);
                item(ui, "Flatten Image", "", Cmd::Flatten, has_doc);
            });
            ui.menu_button("Select", |ui| {
                item(ui, "All", "Ctrl+A", Cmd::SelectAll, has_doc);
                item(ui, "Deselect", "Ctrl+D", Cmd::Deselect, has_sel);
                item(ui, "Inverse", "Shift+Ctrl+I", Cmd::Inverse, has_sel);
                item(ui, "Load Layer Transparency", "Ctrl+click thumb", Cmd::SelectionFromAlpha, has_doc);
                ui.separator();
                ui.menu_button("Modify", |ui| {
                    use dialogs::SelModifyKind::*;
                    item(ui, "Border…", "", Cmd::SelModify(Border), has_sel);
                    item(ui, "Expand…", "", Cmd::SelModify(Expand), has_sel);
                    item(ui, "Contract…", "", Cmd::SelModify(Contract), has_sel);
                    item(ui, "Feather…", "Shift+F6", Cmd::SelModify(Feather), has_sel);
                });
            });
            ui.menu_button("Filter", |ui| {
                let last = self.last_filter.as_ref().map(|f| format!("Last Filter ({})", f.name())).unwrap_or("Last Filter".into());
                item(ui, &last, "Ctrl+F", Cmd::RepeatFilter, has_doc && self.last_filter.is_some());
                ui.separator();
                let mut groups: Vec<&str> = Vec::new();
                for (g, _) in FILTERS {
                    if !groups.contains(&g) {
                        groups.push(g);
                    }
                }
                for g in groups {
                    ui.menu_button(g, |ui| {
                        for (i, (fg, label)) in FILTERS.iter().enumerate() {
                            if *fg == g {
                                item(ui, label, "", Cmd::Filter(i), has_doc);
                            }
                        }
                    });
                }
            });
            ui.menu_button("View", |ui| {
                item(ui, "Zoom In", "Ctrl++", Cmd::ZoomIn, has_doc);
                item(ui, "Zoom Out", "Ctrl+-", Cmd::ZoomOut, has_doc);
                item(ui, "Fit on Screen", "Ctrl+0", Cmd::Fit, has_doc);
                item(ui, "100%", "Ctrl+1", Cmd::ActualPixels, has_doc);
            });
            ui.menu_button("Window", |ui| {
                ui.checkbox(&mut self.show_color, "Color");
                ui.checkbox(&mut self.show_histogram, "Histogram");
                ui.checkbox(&mut self.show_layers, "Layers");
                ui.checkbox(&mut self.show_history, "History");
            });
            ui.menu_button("Help", |ui| {
                item(ui, "Keyboard Shortcuts", "Shift+Ctrl+/", Cmd::Shortcuts, true);
                item(ui, "About Forge Design", "", Cmd::About, true);
            });
        });
        if let Some(c) = cmd {
            let ctx = ui.ctx().clone();
            self.run(c, &ctx);
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
            other => {
                let Some(tab) = self.tabs.get_mut(self.active) else {
                    return;
                };
                match other {
                    Dialog::ImageSize(d) => d.show(ctx, tab),
                    Dialog::CanvasSize(d) => d.show(ctx, tab),
                    Dialog::Adjust(d) => d.show(ctx, tab, &self.tools),
                    Dialog::Filter(d) => d.show(ctx, tab, &self.tools, &mut self.last_filter),
                    Dialog::Transform(d) => d.show(ctx, tab),
                    Dialog::Text(d) => d.show(ctx, tab, &mut self.tools, &self.fonts),
                    Dialog::Fill(d) => d.show(ctx, tab, &self.tools),
                    Dialog::Stroke(d) => d.show(ctx, tab),
                    Dialog::SelModify(d) => d.show(ctx, tab),
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
                            self.tabs.remove(tab);
                            self.active = self.active.min(self.tabs.len().saturating_sub(1));
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
        if self.show_color {
            egui::CollapsingHeader::new("Color").default_open(true).show(ui, |ui| {
                panels::color_panel(ui, &mut self.tools, self.editing_bg, &mut self.hex_buf);
            });
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if self.show_histogram {
            egui::CollapsingHeader::new("Histogram").default_open(true).show(ui, |ui| panels::histogram_panel(ui, tab));
        }
        if self.show_history {
            egui::CollapsingHeader::new("History").default_open(true).show(ui, |ui| {
                ui.set_max_height(150.0);
                panels::history_panel(ui, tab);
            });
        }
        let mut action = None;
        if self.show_layers {
            egui::CollapsingHeader::new("Layers").default_open(true).show(ui, |ui| {
                action = panels::layers_panel(ui, tab, &mut self.layers_ui);
            });
        }
        let ctx = ui.ctx().clone();
        match action {
            Some(LayerAction::New) => self.run(Cmd::NewLayer, &ctx),
            Some(LayerAction::Duplicate) => self.run(Cmd::DuplicateOrViaCopy, &ctx),
            Some(LayerAction::Delete) => self.run(Cmd::DeleteLayer, &ctx),
            Some(LayerAction::AddMask) => self.run(if self.tabs[self.active].doc.selection().is_some() { Cmd::MaskFromSelection } else { Cmd::AddMask }, &ctx),
            Some(LayerAction::MergeDown) => self.run(Cmd::MergeDown, &ctx),
            Some(LayerAction::Up) => self.run(Cmd::LayerUp, &ctx),
            Some(LayerAction::Down) => self.run(Cmd::LayerDown, &ctx),
            Some(LayerAction::SelectionFromAlpha(i)) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    selection_from_alpha(tab, i);
                }
            }
            None => {}
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(tab) = self.tabs.get(self.active) {
                ui.label(format!("{:.1}%", tab.view.zoom * 100.0));
                ui.separator();
                ui.label(format!("{} × {} px", tab.doc.width(), tab.doc.height()));
                ui.separator();
                if let Some(p) = tab.view.hover {
                    let (x, y) = (p.x.floor() as i32, p.y.floor() as i32);
                    if tab.doc.state.bounds().contains(x, y) {
                        ui.label(format!("X: {x}  Y: {y}"));
                        let px = composite::composite_rect(&tab.doc.state, IRect::new(x, y, x + 1, y + 1), composite::Background::Transparent)[0];
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, Color32::from_rgba_unmultiplied(px[0], px[1], px[2], px[3]));
                        ui.label(format!("R {} G {} B {} A {}", px[0], px[1], px[2], px[3]));
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
                ui.label(format!("Layers: {:.1} MB", tiles as f64 * 16.0 / 1024.0));
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

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                ui.label(RichText::new("Forge").size(44.0).strong().color(PAPER));
                ui.label(RichText::new("Design").size(44.0).strong().color(ACCENT));
            });
            ui.label(RichText::new("Open-source image editing, written in Rust").size(15.0).weak());
            ui.add_space(20.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                if ui.add_sized([120.0, 34.0], egui::Button::new("New…")).clicked() {
                    self.run(Cmd::New, &ctx);
                }
                if ui.add_sized([120.0, 34.0], egui::Button::new("Open…")).clicked() {
                    self.run(Cmd::Open, &ctx);
                }
            });
            ui.add_space(12.0);
            ui.label(RichText::new("…or drop image files here (PNG, JPEG, PSD, ORA, WebP, TIFF, …)").weak());
        });
    }
}

fn apply_filter_now(tab: &mut DocTab, f: &Filter) {
    let i = tab.doc.state.active;
    let base = tab.doc.state.layers[i].pixels.clone();
    let sel = tab.doc.state.selection.clone();
    if let Some(r) = forge_core::filters::apply(&mut tab.doc.state.layers[i].pixels, &base, sel.as_ref(), f) {
        tab.doc.mark_dirty(r);
        tab.doc.commit(f.name());
    }
}

fn selection_from_alpha(tab: &mut DocTab, i: usize) {
    let Some(l) = tab.doc.state.layers.get(i) else { return };
    let (w, h) = (tab.doc.width(), tab.doc.height());
    let px = &l.pixels;
    let m = Mask::from_fn_par(w, h, 0, |x, y| px.get(x, y)[3]);
    tab.doc.set_selection(m.content_bounds().map(|_| m));
    tab.doc.commit("Load Selection");
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
            }
        }

        self.handle_shortcuts(ctx);

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
        egui::SidePanel::left("tools").resizable(false).exact_width(84.0).show(ctx, |ui| {
            ui.add_space(4.0);
            panels::toolbar(ui, &mut self.tools, &mut self.editing_bg);
        });
        egui::SidePanel::right("panels").default_width(290.0).min_width(240.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.right_panel(ui));
        });

        egui::CentralPanel::default().frame(egui::Frame::none().fill(INK)).show(ctx, |ui| {
            if self.tabs.is_empty() {
                self.welcome(ui);
                return;
            }
            // Document tabs.
            let mut close = None;
            egui::Frame::none().fill(Color32::from_gray(45)).inner_margin(egui::Margin::symmetric(4.0, 2.0)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, t) in self.tabs.iter().enumerate() {
                        let selected = i == self.active;
                        if ui.selectable_label(selected, t.title()).clicked() {
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

        if let Some(p) = self.tools.text_request.take() {
            if self.dialog.is_none() {
                self.dialog = Some(Dialog::Text(dialogs::TextDialog { pos: p, text: String::new() }));
            }
        }
        self.dialogs(ctx);
    }
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
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
    v.window_rounding = egui::Rounding::same(4.0);
    ctx.set_visuals(v);
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(6.0, 3.0);
    ctx.set_style(style);
}
