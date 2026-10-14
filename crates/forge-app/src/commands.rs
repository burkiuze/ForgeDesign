//! Commands: everything a menu item or shortcut can trigger, the menu bar,
//! and keyboard shortcut handling.

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};
use forge_core::adjust::{self, Adjustment, Channel, Levels};
use forge_core::buffer::TiledBuffer;
use forge_core::filters::Filter;
use forge_core::layer::{Layer, SmartObject};
use forge_core::paths::VectorPath;
use forge_core::selection::{self, Mask};
use forge_core::transform::{self, Orientation};
use forge_core::{composite, fill, BlendMode, Color, IRect, Point, Rgba8};

use crate::actions::ActionStep;
use crate::app::{Dialog, ForgeApp};
use crate::canvas::{DocTab, Proof};
use crate::dialogs::{self, SelModifyKind};
use crate::tools::{Tool, TransformMode, GROUPS};

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AlignKind {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Cmd {
    // File
    New,
    Open,
    Place,
    Save,
    SaveAs,
    Export,
    QuickExportPng,
    ExportPdf,
    ExportLayers,
    ExportGif,
    Batch,
    Revert,
    Close,
    Quit,
    // Edit
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
    ContentAwareFill,
    Stroke,
    Transform(TransformModeSer),
    TransformNumeric,
    DefineBrush,
    DefinePattern,
    // Image
    Grayscale,
    ImageSize,
    CanvasSize,
    Orient(Orientation),
    CropToSelection,
    Trim,
    Flatten,
    AutoContrast,
    AutoTone,
    AutoColor,
    Adjust(usize),
    // Layer
    NewLayer,
    NewGroup,
    GroupLayers,
    Ungroup,
    DuplicateOrViaCopy,
    LayerViaCut,
    DeleteLayer,
    MergeDown,
    MergeVisible,
    StampVisible,
    LayerUp,
    LayerDown,
    LayerTop,
    LayerBottom,
    AddMask,
    HideAllMask,
    MaskFromSelection,
    MaskFromSelectionOrReveal,
    DeleteMask,
    ApplyMask,
    InvertMask,
    ToggleLockAlpha,
    ToggleClip,
    LayerStyle,
    CopyStyle,
    PasteStyle,
    ClearStyle,
    NewAdjLayer(usize),
    NewFillLayer(u8),
    ConvertSmart,
    Rasterize,
    Align(AlignKind),
    // Select
    SelectAll,
    Deselect,
    Reselect,
    Inverse,
    ColorRange,
    FocusArea,
    SelModify(SelModifyKindSer),
    SelectionFromAlpha,
    SaveSelection,
    LoadSelection(usize),
    MakeWorkPath,
    // Filter
    Filter(usize),
    RepeatFilter,
    Liquify,
    // View
    ZoomIn,
    ZoomOut,
    Fit,
    ActualPixels,
    ToggleRulers,
    ToggleGrid,
    ToggleGuides,
    ToggleSnap,
    NewGuide,
    ClearGuides,
    Proof(ProofSer),
    Fullscreen,
    TogglePanels,
    ShowPanel(crate::app::PanelId),
    AddFont,
    CloseAll,
    CheckUpdates,
    UpdateFromSource,
    ToggleTheme,
    // Help
    About,
    Shortcuts,
}

/// Serde-friendly wrappers (actions only record parameter-free commands).
#[derive(Clone, Copy, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct TransformModeSer(pub u8);
#[derive(Clone, Copy, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SelModifyKindSer(pub u8);
#[derive(Clone, Copy, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ProofSer(pub u8);

impl TransformModeSer {
    pub fn of(m: TransformMode) -> Self {
        Self(match m {
            TransformMode::Free => 0,
            TransformMode::Skew => 1,
            TransformMode::Distort => 2,
            TransformMode::Perspective => 3,
            TransformMode::Warp => 4,
        })
    }
    pub fn mode(self) -> TransformMode {
        match self.0 {
            1 => TransformMode::Skew,
            2 => TransformMode::Distort,
            3 => TransformMode::Perspective,
            4 => TransformMode::Warp,
            _ => TransformMode::Free,
        }
    }
}

impl SelModifyKindSer {
    pub fn of(k: SelModifyKind) -> Self {
        Self(match k {
            SelModifyKind::Feather => 0,
            SelModifyKind::Expand => 1,
            SelModifyKind::Contract => 2,
            SelModifyKind::Border => 3,
        })
    }
    pub fn kind(self) -> SelModifyKind {
        match self.0 {
            1 => SelModifyKind::Expand,
            2 => SelModifyKind::Contract,
            3 => SelModifyKind::Border,
            _ => SelModifyKind::Feather,
        }
    }
}

impl ProofSer {
    pub fn of(p: Proof) -> Self {
        Self(Proof::ALL.iter().position(|x| *x == p).unwrap_or(0) as u8)
    }
    pub fn proof(self) -> Proof {
        Proof::ALL.get(self.0 as usize).copied().unwrap_or_default()
    }
}

/// Adjustment menu entries: (label, shortcut). Indices are stable ids.
pub const ADJUSTMENTS: [(&str, &str); 19] = [
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
    ("Channel Mixer…", ""),
    ("Selective Color…", ""),
    ("Color Temperature / White Balance…", ""),
    ("Color Lookup (.cube)…", ""),
    ("Shadows/Highlights…", ""),
];

/// Menu order with separators (usize::MAX).
const ADJ_MENU: [usize; 23] = [0, 1, 2, 3, usize::MAX, 4, 5, 6, 7, 8, 14, 17, 16, usize::MAX, 10, 11, 12, 9, 15, usize::MAX, 18, 13, usize::MAX];

pub fn default_adjustment(i: usize, fg: Color, bg: Color) -> Adjustment {
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
        14 => Adjustment::ChannelMixer {
            matrix: [[100.0, 0.0, 0.0, 0.0], [0.0, 100.0, 0.0, 0.0], [0.0, 0.0, 100.0, 0.0]],
            monochrome: false,
        },
        15 => Adjustment::SelectiveColor { shifts: [[0.0; 4]; 9], absolute: false },
        16 => Adjustment::WhiteBalance { temperature: 0.0, tint: 0.0 },
        17 => Adjustment::ColorLookup { lut: std::sync::Arc::new(identity_lut()) },
        _ => Adjustment::Desaturate,
    }
}

fn identity_lut() -> adjust::Lut3d {
    let n = 2;
    let mut data = Vec::new();
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                data.push([r as f32, g as f32, b as f32]);
            }
        }
    }
    adjust::Lut3d { name: "Identity".into(), size: n, data }
}

/// Filter menu: (submenu, label).
pub const FILTERS: [(&str, &str); 36] = [
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
    ("Blur", "Radial Blur…"),
    ("Blur", "Lens Blur…"),
    ("Blur", "Surface Blur…"),
    ("Blur Gallery", "Tilt-Shift…"),
    ("Sharpen", "Smart Sharpen…"),
    ("Sharpen", "Sharpen Edges"),
    ("Noise", "Reduce Noise…"),
    ("Noise", "Dust & Scratches…"),
    ("Stylize", "Oil Paint…"),
    ("Distort", "Ripple…"),
    ("Distort", "Spherize…"),
    ("Distort", "Polar Coordinates…"),
    ("Lens", "Lens Correction…"),
    ("Adjust", "Shadows/Highlights…"),
    ("Adjust", "HDR Toning…"),
    ("Other", "Offset…"),
    ("Distort", "Displace…"),
    ("Blur Gallery", "Field Blur…"),
    ("Blur", "Shape Blur…"),
    ("Distort", "Pinch…"),
    ("Render", "Vignette…"),
    ("Blur Gallery", "Spin Blur…"),
];

pub fn default_filter(i: usize, fg: Color, bg: Color) -> Filter {
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
        14 => Filter::RadialBlur { amount: 30.0, zoom: true },
        15 | 32 => Filter::LensBlur { radius: 8.0 },
        16 => Filter::SurfaceBlur { radius: 5, threshold: 15.0 },
        17 => Filter::TiltShift { center: 0.5, band: 0.2, blur: 12.0 },
        31 => Filter::GaussianBlur { radius: 15.0 },
        18 => Filter::SmartSharpen { amount: 1.5, radius: 1.0, noise: 10.0 },
        19 => Filter::SharpenEdges,
        20 => Filter::ReduceNoise { strength: 0.4 },
        21 => Filter::DustAndScratches { radius: 2, threshold: 20 },
        22 => Filter::OilPaint { radius: 4 },
        23 => Filter::Ripple { amount: 6.0, size: 18.0 },
        24 => Filter::Spherize { amount: 50.0 },
        33 => Filter::Spherize { amount: -50.0 },
        25 => Filter::PolarCoordinates { to_polar: true },
        26 => Filter::LensCorrection { distortion: 0.0, vignette: 0.0, chromatic: 0.0 },
        34 => Filter::LensCorrection { distortion: 0.0, vignette: -50.0, chromatic: 0.0 },
        27 => Filter::ShadowsHighlights { shadows: 35.0, highlights: 0.0, radius: 30.0 },
        28 => Filter::HdrToning { strength: 0.6, radius: 30.0, saturation: 20.0 },
        29 => Filter::Offset { dx: 50, dy: 50 },
        30 => Filter::Displace { amount: 10.0, scale: 60.0, seed: 3 },
        35 => Filter::RadialBlur { amount: 10.0, zoom: false },
        _ => Filter::Clouds { fg, bg, scale: 200.0, seed: 7 },
    }
}

fn instant_filter(f: &Filter) -> bool {
    matches!(f, Filter::Sharpen | Filter::FindEdges | Filter::SharpenEdges)
}

fn instant_adjustment(a: &Adjustment) -> bool {
    matches!(a, Adjustment::Invert | Adjustment::Desaturate | Adjustment::BlackAndWhite)
}

/// Commands that open dialogs or have side effects outside the document
/// and must not be replayed by Actions.
fn recordable(c: Cmd) -> bool {
    !matches!(
        c,
        Cmd::New
            | Cmd::Open
            | Cmd::Place
            | Cmd::Save
            | Cmd::SaveAs
            | Cmd::Export
            | Cmd::ExportPdf
            | Cmd::ExportLayers
            | Cmd::ExportGif
            | Cmd::QuickExportPng
            | Cmd::Batch
            | Cmd::Close
            | Cmd::Quit
            | Cmd::Undo
            | Cmd::Redo
            | Cmd::About
            | Cmd::Shortcuts
            | Cmd::ZoomIn
            | Cmd::ZoomOut
            | Cmd::Fit
            | Cmd::ActualPixels
            | Cmd::Fullscreen
            | Cmd::TogglePanels
            | Cmd::ShowPanel(_)
            | Cmd::AddFont
            | Cmd::CheckUpdates
            | Cmd::UpdateFromSource
            | Cmd::CloseAll
            | Cmd::ToggleTheme
            | Cmd::ToggleRulers
            | Cmd::ToggleGrid
            | Cmd::ToggleGuides
            | Cmd::ToggleSnap
            | Cmd::Proof(_)
            | Cmd::FillDialog
            | Cmd::Stroke
            | Cmd::TransformNumeric
            | Cmd::Transform(_)
            | Cmd::ImageSize
            | Cmd::CanvasSize
            | Cmd::LayerStyle
            | Cmd::ColorRange
            | Cmd::FocusArea
            | Cmd::SelModify(_)
            | Cmd::NewGuide
            | Cmd::Liquify
    ) && !matches!(c, Cmd::Adjust(_) | Cmd::Filter(_))
}

pub fn apply_filter_now(tab: &mut DocTab, f: &Filter) {
    let i = tab.doc.state.active;
    // Smart objects get the filter as an editable Smart Filter.
    if let Some(so) = tab.doc.state.layers[i].smart.as_mut() {
        so.filters.push(forge_core::layer::SmartFilter { filter: f.clone(), enabled: true });
        tab.doc.state.layers[i].pixels = so.render();
        tab.doc.mark_all_dirty();
        tab.doc.commit(format!("Smart Filter: {}", f.name()));
        return;
    }
    let base = tab.doc.state.layers[i].pixels.clone();
    let sel = tab.doc.state.selection.clone();
    if let Some(r) = forge_core::filters::apply(&mut tab.doc.state.layers[i].pixels, &base, sel.as_ref(), f) {
        tab.doc.mark_dirty(r);
        tab.doc.commit(f.name());
    }
}

pub fn apply_adjustment_now(tab: &mut DocTab, adj: &Adjustment) {
    let layer = tab.doc.state.active;
    let Some(l) = tab.doc.state.layers.get(layer) else { return };
    if !l.has_pixels() {
        return;
    }
    let base = l.pixels.clone();
    let sel = tab.doc.state.selection.clone();
    if let Some(r) = adjust::apply(&mut tab.doc.state.layers[layer].pixels, &base, sel.as_ref(), adj) {
        tab.doc.mark_dirty(r);
        tab.doc.commit(adj.name());
    }
}

pub fn selection_from_alpha(tab: &mut DocTab, i: usize) {
    let Some(l) = tab.doc.state.layers.get(i) else { return };
    let (w, h) = (tab.doc.width(), tab.doc.height());
    let px = &l.pixels;
    let m = Mask::from_fn_par(w, h, 0, |x, y| px.get(x, y)[3]);
    tab.doc.set_selection(m.content_bounds().map(|_| m));
    tab.doc.commit("Load Selection");
}

fn auto_adjust(tab: &mut DocTab, label: &str, per_channel: bool) {
    let (w, h) = (tab.doc.width(), tab.doc.height());
    let layer = tab.doc.state.active;
    if !tab.doc.state.layers[layer].has_pixels() {
        return;
    }
    let base = tab.doc.state.layers[layer].pixels.clone();
    let sel = tab.doc.state.selection.clone();
    let rect = selection::work_rect(sel.as_ref(), w, h);
    let mut adjs = adjust::auto_levels(&base, rect);
    if !per_channel {
        // Auto Contrast/Tone: one stretch for all channels (keeps colour balance).
        let lo = adjs.iter().filter_map(|a| match a {
            Adjustment::Levels { levels, .. } => Some(levels.in_black),
            _ => None,
        });
        let lo = lo.fold(255.0f32, f32::min);
        let hi = adjs
            .iter()
            .filter_map(|a| match a {
                Adjustment::Levels { levels, .. } => Some(levels.in_white),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        adjs = if hi > lo {
            vec![Adjustment::Levels { channel: Channel::Rgb, levels: Levels { in_black: lo, in_white: hi, ..Default::default() } }]
        } else {
            vec![]
        };
    }
    for adj in adjs {
        let b = tab.doc.state.layers[layer].pixels.clone();
        adjust::apply(&mut tab.doc.state.layers[layer].pixels, &b, sel.as_ref(), &adj);
    }
    tab.doc.mark_dirty(rect);
    tab.doc.commit(label);
}

fn align_layer(tab: &mut DocTab, kind: AlignKind) {
    let (w, h) = (tab.doc.width() as i32, tab.doc.height() as i32);
    let target = tab.doc.selection().and_then(|s| s.content_bounds()).unwrap_or(IRect::new(0, 0, w, h));
    let Some(l) = tab.doc.state.active_layer() else { return };
    if !l.has_pixels() || l.locked {
        return;
    }
    let Some(b) = l.pixels.content_bounds() else { return };
    let (dx, dy) = match kind {
        AlignKind::Left => (target.x0 - b.x0, 0),
        AlignKind::HCenter => ((target.x0 + target.x1) / 2 - (b.x0 + b.x1) / 2, 0),
        AlignKind::Right => (target.x1 - b.x1, 0),
        AlignKind::Top => (0, target.y0 - b.y0),
        AlignKind::VCenter => (0, (target.y0 + target.y1) / 2 - (b.y0 + b.y1) / 2),
        AlignKind::Bottom => (0, target.y1 - b.y1),
    };
    if (dx, dy) == (0, 0) {
        return;
    }
    let px = transform::translate(&l.pixels, dx, dy, w as u32, h as u32);
    let mask = l.mask.as_ref().map(|m| transform::translate(m, dx, dy, w as u32, h as u32));
    let l = tab.doc.state.active_layer_mut().unwrap();
    l.pixels = px;
    if mask.is_some() {
        l.mask = mask;
    }
    tab.doc.mark_all_dirty();
    tab.doc.commit("Align");
}

impl ForgeApp {
    pub fn run(&mut self, cmd: Cmd, ctx: &egui::Context) {
        if self.tools.is_busy() {
            return;
        }
        if self.recorder.recording && recordable(cmd) {
            self.recorder.push(ActionStep::Cmd(cmd));
        }
        let (fg, bg) = (self.tools.fg, self.tools.bg);
        // Commands that don't need an open document.
        match cmd {
            Cmd::New => {
                let mut nd = dialogs::NewDoc::default();
                if let Some(c) = &self.clipboard {
                    nd.width = c.pixels.width();
                    nd.height = c.pixels.height();
                }
                nd.name = format!("Untitled-{}", self.tabs.len() + 1);
                self.dialog = Some(Dialog::New(nd));
                return;
            }
            Cmd::Open => return self.open_dialog(),
            Cmd::Place => return self.place(),
            Cmd::Save => {
                self.save(false);
                return;
            }
            Cmd::SaveAs => {
                self.save(true);
                return;
            }
            Cmd::Export => return self.export(None),
            Cmd::QuickExportPng => return self.export(Some("png")),
            Cmd::ExportPdf => return self.export(Some("pdf")),
            Cmd::ExportLayers => return self.export_layers(),
            Cmd::ExportGif => {
                if !self.tabs.is_empty() {
                    self.dialog = Some(Dialog::Animation(dialogs::AnimationDialog::default()));
                }
                return;
            }
            Cmd::Batch => {
                self.dialog = Some(Dialog::Batch(dialogs::BatchDialog::default()));
                return;
            }
            Cmd::Close => return self.request_close_tab(self.active),
            Cmd::Quit => return ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Cmd::About => {
                self.dialog = Some(Dialog::About);
                return;
            }
            Cmd::Shortcuts => {
                self.dialog = Some(Dialog::Shortcuts);
                return;
            }
            Cmd::Copy => return self.copy(false, false),
            Cmd::CopyMerged => return self.copy(true, false),
            Cmd::Cut => return self.copy(false, true),
            Cmd::Paste => {
                if self.tabs.is_empty() {
                    self.paste_as_new_document();
                } else {
                    self.paste();
                }
                return;
            }
            Cmd::Clear => return self.clear_selection_pixels("Clear"),
            Cmd::Fullscreen => {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
                return;
            }
            Cmd::ShowPanel(id) => {
                self.hide_panels = false;
                self.panels.focus(id);
                return;
            }
            Cmd::AddFont => return self.add_font_dialog(),
            Cmd::CheckUpdates => return self.updater.check(false),
            Cmd::UpdateFromSource => return self.updater.update_from_source(),
            Cmd::CloseAll => {
                for i in (0..self.tabs.len()).rev() {
                    self.request_close_tab(i);
                    if self.dialog.is_some() {
                        break;
                    }
                }
                return;
            }
            Cmd::TogglePanels => {
                self.hide_panels = !self.hide_panels;
                return;
            }
            Cmd::ToggleTheme => {
                self.light_theme = !self.light_theme;
                crate::app::setup_style(ctx, self.light_theme);
                return;
            }
            Cmd::Liquify => {
                self.tools.select_tool(Tool::Liquify);
                return;
            }
            Cmd::DefinePattern | Cmd::DefineBrush => {
                let Some(tab) = self.tabs.get(self.active) else { return };
                let Some(l) = tab.doc.state.active_layer() else { return };
                let rect = match tab.doc.selection() {
                    Some(s) => s.content_bounds(),
                    None => l.pixels.content_bounds(),
                };
                let Some(rect) = rect else { return };
                if cmd == Cmd::DefinePattern {
                    let px = l.pixels.region_to_dense(rect);
                    self.tools.pattern = std::sync::Arc::new(TiledBuffer::from_dense(rect.width() as u32, rect.height() as u32, &px));
                    self.tools.status = format!("Pattern defined ({}×{})", rect.width(), rect.height());
                } else if let Some(tip) = forge_core::brush::tip_from_buffer(&l.pixels, rect) {
                    let tip = std::sync::Arc::new(tip);
                    self.brush_presets.push(crate::panels::BrushPreset {
                        name: format!("Custom {}", self.brush_presets.len() + 1),
                        settings: forge_core::brush::BrushSettings { size: rect.width().max(rect.height()) as f32, ..Default::default() },
                        tip: Some(tip.clone()),
                    });
                    self.tools.tip = Some(tip);
                    self.tools.select_tool(Tool::Brush);
                    self.tools.status = "Brush preset defined".into();
                }
                return;
            }
            _ => {}
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let (w, h) = (tab.doc.width(), tab.doc.height());
        let has_pixels = tab.doc.state.active_layer().is_some_and(|l| l.has_pixels());
        match cmd {
            Cmd::Undo => {
                tab.doc.undo();
            }
            Cmd::Redo => {
                tab.doc.redo();
            }
            Cmd::Revert => {
                if tab.doc.history().cursor() > 0 {
                    tab.doc.jump_history(0);
                    tab.doc.commit("Revert");
                }
            }
            Cmd::FillDialog => self.dialog = Some(Dialog::Fill(dialogs::FillDialog::new(&self.tools))),
            Cmd::FillFg => dialogs::fill_selection(tab, fg, BlendMode::Normal, 1.0, false, "Fill"),
            Cmd::FillBg => dialogs::fill_selection(tab, bg, BlendMode::Normal, 1.0, false, "Fill"),
            Cmd::ContentAwareFill => {
                let Some(sel) = tab.doc.state.selection.clone() else {
                    self.tools.status = "Select the area to fill first".into();
                    return;
                };
                if !has_pixels {
                    return;
                }
                let l = tab.doc.state.active_layer_mut().unwrap();
                let base = l.pixels.clone();
                if let Some(r) = forge_core::retouch::content_aware_fill(&mut l.pixels, &base, &sel) {
                    tab.doc.mark_dirty(r.expand(4));
                    tab.doc.commit("Content-Aware Fill");
                }
            }
            Cmd::Stroke => {
                if tab.doc.selection().is_some() {
                    self.dialog = Some(Dialog::Stroke(dialogs::StrokeDialog::new(&self.tools)));
                }
            }
            Cmd::Transform(m) => {
                if let Err(e) = self.tools.begin_transform(tab, m.mode()) {
                    self.tools.status = e;
                }
            }
            Cmd::TransformNumeric => {
                if let Some(d) = dialogs::TransformDialog::new(tab) {
                    self.dialog = Some(Dialog::Transform(Box::new(d)));
                }
            }
            Cmd::Grayscale => {
                let adj = Adjustment::BlackAndWhite;
                for l in tab.doc.state.layers.iter_mut().filter(|l| l.has_pixels()) {
                    let base = l.pixels.clone();
                    adjust::apply(&mut l.pixels, &base, None, &adj);
                }
                tab.doc.mark_all_dirty();
                tab.doc.commit("Grayscale");
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
            Cmd::AutoContrast if has_pixels => auto_adjust(tab, "Auto Contrast", false),
            Cmd::AutoTone if has_pixels => auto_adjust(tab, "Auto Tone", false),
            Cmd::AutoColor if has_pixels => auto_adjust(tab, "Auto Color", true),
            Cmd::Adjust(i) if has_pixels => {
                let adj = default_adjustment(i, fg, bg);
                if instant_adjustment(&adj) {
                    apply_adjustment_now(tab, &adj);
                    if self.recorder.recording {
                        self.recorder.push(ActionStep::Adjust(adj));
                    }
                } else if let Some(d) = dialogs::AdjustDialog::new(tab, adj) {
                    self.dialog = Some(Dialog::Adjust(Box::new(d)));
                }
            }
            Cmd::Filter(i) if has_pixels => {
                let f = default_filter(i, fg, bg);
                if instant_filter(&f) {
                    apply_filter_now(tab, &f);
                    if self.recorder.recording {
                        self.recorder.push(ActionStep::Filter(f.clone()));
                    }
                    self.last_filter = Some(f);
                } else if let Some(d) = dialogs::FilterDialog::new(tab, f) {
                    self.dialog = Some(Dialog::Filter(Box::new(d)));
                }
            }
            Cmd::RepeatFilter if has_pixels => {
                if let Some(f) = self.last_filter.clone() {
                    apply_filter_now(tab, &f);
                }
            }
            Cmd::NewLayer => tab.doc.new_layer(),
            Cmd::NewGroup => tab.doc.new_group(),
            Cmd::GroupLayers => tab.doc.group_layer(),
            Cmd::Ungroup => tab.doc.ungroup(),
            Cmd::DuplicateOrViaCopy | Cmd::LayerViaCut => {
                let cut = cmd == Cmd::LayerViaCut;
                if let (Some(sel), Some(l)) = (tab.doc.state.selection.clone(), tab.doc.state.active_layer().filter(|l| l.has_pixels())) {
                    let mut px = l.pixels.clone();
                    let name = format!("{} ({})", l.name, if cut { "cut" } else { "copy" });
                    px.par_map_all(|x, y, mut p| {
                        p[3] = (p[3] as f32 * selection::coverage(Some(&sel), x, y) + 0.5) as u8;
                        p
                    });
                    px.optimize();
                    if cut {
                        let l = tab.doc.state.active_layer_mut().unwrap();
                        fill::clear(&mut l.pixels, Some(&sel));
                    }
                    let id = tab.doc.state.alloc_id();
                    tab.doc.add_layer(Layer::with_pixels(id, name, px), if cut { "Layer via Cut" } else { "Layer via Copy" });
                } else if !cut {
                    tab.doc.duplicate_layer();
                }
            }
            Cmd::DeleteLayer => tab.doc.delete_layer(),
            Cmd::MergeDown => tab.doc.merge_down(),
            Cmd::MergeVisible => {
                let pixels = composite::merged(&tab.doc.state);
                let id = tab.doc.state.alloc_id();
                tab.doc.state.layers.retain(|l| !l.visible);
                tab.doc.state.layers.push(Layer::with_pixels(id, "Merged", pixels));
                tab.doc.state.active = tab.doc.state.layers.len() - 1;
                tab.doc.mark_all_dirty();
                tab.doc.commit("Merge Visible");
            }
            Cmd::StampVisible => tab.doc.stamp_visible(),
            Cmd::LayerUp => tab.doc.move_layer(1),
            Cmd::LayerDown => tab.doc.move_layer(-1),
            Cmd::LayerTop => tab.doc.move_layer(i32::MAX / 2),
            Cmd::LayerBottom => tab.doc.move_layer(-i32::MAX / 2),
            Cmd::AddMask => tab.doc.add_layer_mask(false),
            Cmd::HideAllMask => {
                tab.doc.add_layer_mask(false);
                if let Some(m) = tab.doc.state.active_layer_mut().and_then(|l| l.mask.as_mut()) {
                    *m = TiledBuffer::new_filled(w, h, 0);
                    tab.doc.mark_all_dirty();
                    tab.doc.commit("Hide All");
                }
            }
            Cmd::MaskFromSelection => tab.doc.add_layer_mask(true),
            Cmd::MaskFromSelectionOrReveal => tab.doc.add_layer_mask(tab.doc.state.selection.is_some()),
            Cmd::DeleteMask => tab.doc.delete_layer_mask(false),
            Cmd::ApplyMask => tab.doc.delete_layer_mask(true),
            Cmd::InvertMask => {
                if let Some(m) = tab.doc.state.active_layer_mut().and_then(|l| l.mask.as_mut()) {
                    let fill_v = 255 - m.fill_value();
                    let mut inv = TiledBuffer::new_filled(w, h, fill_v);
                    let src = m.clone();
                    inv.par_map_all(|x, y, _| 255 - src.get(x, y));
                    inv.optimize();
                    *m = inv;
                    tab.doc.mark_all_dirty();
                    tab.doc.commit("Invert Mask");
                }
            }
            Cmd::ToggleLockAlpha => {
                if let Some(l) = tab.doc.state.active_layer_mut() {
                    l.lock_alpha = !l.lock_alpha;
                    tab.doc.commit("Lock Change");
                }
            }
            Cmd::ToggleClip => {
                let i = tab.doc.state.active;
                if i > 0 {
                    let l = &mut tab.doc.state.layers[i];
                    l.clipped = !l.clipped;
                    let label = if l.clipped { "Create Clipping Mask" } else { "Release Clipping Mask" };
                    tab.doc.mark_all_dirty();
                    tab.doc.commit(label);
                }
            }
            Cmd::LayerStyle => {
                if let Some(d) = dialogs::LayerStyleDialog::new(tab) {
                    self.dialog = Some(Dialog::LayerStyle(Box::new(d)));
                }
            }
            Cmd::CopyStyle => self.style_clipboard = tab.doc.state.active_layer().and_then(|l| l.style.clone()),
            Cmd::PasteStyle => {
                if let Some(st) = self.style_clipboard.clone() {
                    if let Some(l) = tab.doc.state.active_layer_mut() {
                        l.style = Some(st);
                        tab.doc.mark_all_dirty();
                        tab.doc.commit("Paste Layer Style");
                    }
                }
            }
            Cmd::ClearStyle => {
                if let Some(l) = tab.doc.state.active_layer_mut() {
                    if l.style.take().is_some() {
                        tab.doc.mark_all_dirty();
                        tab.doc.commit("Clear Layer Style");
                    }
                }
            }
            Cmd::NewAdjLayer(i) => {
                let adj = default_adjustment(i, fg, bg);
                let id = tab.doc.state.alloc_id();
                let mut l = Layer::new_adjustment(id, adj, w, h);
                if let Some(sel) = tab.doc.state.selection.clone() {
                    l.mask = Some(sel);
                }
                l.parent = tab.doc.state.active_layer().and_then(|a| a.parent);
                tab.doc.add_layer(l, "New Adjustment Layer");
                self.panels.focus(crate::app::PanelId::Properties);
            }
            Cmd::NewFillLayer(kind) => {
                let id = tab.doc.state.alloc_id();
                let (name, px) = match kind {
                    0 => ("Color Fill", TiledBuffer::solid(w, h, fg.to_rgba8())),
                    1 => {
                        let mut px = TiledBuffer::new(w, h);
                        fill::gradient(
                            &mut px,
                            None,
                            fill::GradientKind::Linear,
                            Point::new(0.0, 0.0),
                            Point::new(w as f32, h as f32),
                            fg,
                            bg,
                            1.0,
                            BlendMode::Normal,
                            false,
                        );
                        ("Gradient Fill", px)
                    }
                    _ => {
                        let pat = self.tools.pattern.clone();
                        let (pw, ph) = (pat.width() as i32, pat.height() as i32);
                        ("Pattern Fill", TiledBuffer::from_fn_par(w, h, [0; 4], |x, y| pat.get(x.rem_euclid(pw), y.rem_euclid(ph))))
                    }
                };
                let mut l = Layer::with_pixels(id, name, px);
                if let Some(sel) = tab.doc.state.selection.clone() {
                    l.mask = Some(sel);
                }
                tab.doc.add_layer(l, "New Fill Layer");
            }
            Cmd::ConvertSmart => {
                if let Some(l) = tab.doc.state.active_layer_mut().filter(|l| l.has_pixels() && l.smart.is_none()) {
                    l.smart = Some(Box::new(SmartObject::new(l.pixels.clone())));
                    tab.doc.commit("Convert to Smart Object");
                }
            }
            Cmd::Rasterize => {
                if let Some(l) = tab.doc.state.active_layer_mut() {
                    if l.smart.take().is_some() || l.text.take().is_some() {
                        tab.doc.commit("Rasterize Layer");
                    }
                }
            }
            Cmd::Align(k) => align_layer(tab, k),
            Cmd::SelectAll => {
                tab.doc.set_selection(Some(selection::select_all(w, h)));
                tab.doc.commit("Select All");
            }
            Cmd::Deselect => {
                if let Some(s) = tab.doc.state.selection.clone() {
                    tab.last_selection = Some(s);
                    tab.doc.set_selection(None);
                    tab.doc.commit("Deselect");
                }
            }
            Cmd::Reselect => {
                if let Some(s) = tab.last_selection.clone() {
                    if s.width() == w && s.height() == h {
                        tab.doc.set_selection(Some(s));
                        tab.doc.commit("Reselect");
                    }
                }
            }
            Cmd::Inverse => {
                let inv = selection::invert(tab.doc.selection(), w, h);
                if tab.doc.selection().is_some() {
                    tab.doc.set_selection(inv);
                    tab.doc.commit("Inverse");
                }
            }
            Cmd::ColorRange => self.dialog = Some(Dialog::ColorRange(dialogs::ColorRangeDialog::new(tab, fg))),
            Cmd::FocusArea => self.dialog = Some(Dialog::FocusArea(dialogs::FocusAreaDialog::new(tab))),
            Cmd::SelModify(kind) => {
                if tab.doc.selection().is_some() {
                    let kind = kind.kind();
                    let amount = if kind == SelModifyKind::Feather { 5.0 } else { 3.0 };
                    self.dialog = Some(Dialog::SelModify(dialogs::SelModify { kind, amount }));
                }
            }
            Cmd::SelectionFromAlpha => {
                let i = tab.doc.state.active;
                selection_from_alpha(tab, i);
            }
            Cmd::SaveSelection => {
                if let Some(s) = tab.doc.state.selection.clone() {
                    let n = tab.channels.len() + 1;
                    tab.channels.push((format!("Alpha {n}"), s));
                    self.tools.status = format!("Selection saved as Alpha {n}");
                }
            }
            Cmd::LoadSelection(i) => {
                if let Some((_, s)) = tab.channels.get(i).cloned() {
                    if s.width() == w && s.height() == h {
                        tab.doc.set_selection(Some(s));
                        tab.doc.commit("Load Selection");
                    }
                }
            }
            Cmd::MakeWorkPath => {
                if let Some(s) = tab.doc.selection() {
                    let mut p = VectorPath::from_selection(s);
                    p.name = format!("Path {}", tab.doc.state.paths.len() + 1);
                    tab.doc.state.paths.push(p);
                    tab.doc.state.active_path = Some(tab.doc.state.paths.len() - 1);
                    tab.doc.commit("Make Work Path");
                    self.panels.focus(crate::app::PanelId::Paths);
                }
            }
            Cmd::ZoomIn => tab.view.zoom_step(1, None, w, h),
            Cmd::ZoomOut => tab.view.zoom_step(-1, None, w, h),
            Cmd::Fit => tab.view.request_fit(),
            Cmd::ActualPixels => {
                let c = tab.view.canvas_rect.center();
                tab.view.zoom_to(1.0, c, w, h);
            }
            Cmd::ToggleRulers => tab.view.show_rulers = !tab.view.show_rulers,
            Cmd::ToggleGrid => tab.view.show_grid = !tab.view.show_grid,
            Cmd::ToggleGuides => tab.view.show_guides = !tab.view.show_guides,
            Cmd::ToggleSnap => tab.view.snap = !tab.view.snap,
            Cmd::ClearGuides => tab.view.guides.clear(),
            Cmd::NewGuide => self.dialog = Some(Dialog::NewGuide(dialogs::NewGuideDialog::default())),
            Cmd::Proof(p) => {
                let p = p.proof();
                tab.view.proof = if tab.view.proof == p { Proof::Off } else { p };
            }
            _ => {}
        }
    }

    pub fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        use Key::*;
        let cmd = Modifiers::COMMAND;
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        let ca = Modifiers::COMMAND | Modifiers::ALT;
        let cas = Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT;
        let free = Cmd::Transform(TransformModeSer::of(TransformMode::Free));
        // Most specific modifier combinations first: egui matches "logically".
        let a = Modifiers::ALT;
        let s = Modifiers::SHIFT;
        let n = Modifiers::NONE;
        let feather = Cmd::SelModify(SelModifyKindSer::of(SelModifyKind::Feather));
        let cmyk = Cmd::Proof(ProofSer::of(Proof::Cmyk));
        // Adobe Photoshop default keyboard shortcuts (Windows; Ctrl = Cmd on macOS).
        // Most specific modifier combinations first: egui matches "logically".
        let table: Vec<(Modifiers, Key, Cmd)> = vec![
            // Alt+Shift+Ctrl
            (cas, B, Cmd::Adjust(7)),           // Black & White
            (cas, E, Cmd::StampVisible),        // Stamp visible
            (cas, N, Cmd::NewLayer),            // New layer (no dialog)
            (cas, L, Cmd::AutoContrast),        // Auto Contrast
            (cas, K, Cmd::Shortcuts),           // Keyboard Shortcuts
            (cas, I, Cmd::ImageSize),           // (File Info in PS; harmless)
            (cas, S, Cmd::Export),              // Save for Web
            (cas, W, Cmd::Export),              // Export As
            (cas, Z, Cmd::Undo),
            // Shift+Ctrl
            (cs, Z, Cmd::Redo),
            (cs, S, Cmd::SaveAs),
            (cs, N, Cmd::NewLayer),
            (cs, E, Cmd::MergeVisible),
            (cs, I, Cmd::Inverse),
            (cs, C, Cmd::CopyMerged),
            (cs, V, Cmd::Paste),                // Paste in Place
            (cs, U, Cmd::Adjust(13)),           // Desaturate
            (cs, L, Cmd::AutoTone),
            (cs, B, Cmd::AutoColor),
            (cs, G, Cmd::Ungroup),
            (cs, J, Cmd::LayerViaCut),
            (cs, D, Cmd::Reselect),
            (cs, X, Cmd::Liquify),
            (cs, A, Cmd::Filter(26)),           // Camera Raw-like lens/lighting
            (cs, CloseBracket, Cmd::LayerTop),
            (cs, OpenBracket, Cmd::LayerBottom),
            (cs, Semicolon, Cmd::ToggleSnap),
            // Alt+Ctrl
            (ca, I, Cmd::ImageSize),
            (ca, C, Cmd::CanvasSize),
            (ca, G, Cmd::ToggleClip),
            (ca, W, Cmd::CloseAll),
            (ca, Num0, Cmd::ActualPixels),
            (ca, Z, Cmd::Undo),
            (ca, R, feather),                   // Select and Mask
            (ca, D, feather),                   // Feather (legacy)
            (ca, E, Cmd::StampVisible),         // Merge (copy) to new layer
            (ca, F, Cmd::RepeatFilter),
            // Ctrl
            (cmd, Z, Cmd::Undo),
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
            (cmd, G, Cmd::GroupLayers),
            (cmd, T, free),
            (cmd, L, Cmd::Adjust(1)),           // Levels
            (cmd, M, Cmd::Adjust(2)),           // Curves
            (cmd, U, Cmd::Adjust(5)),           // Hue/Saturation
            (cmd, B, Cmd::Adjust(6)),           // Color Balance
            (cmd, I, Cmd::Adjust(10)),          // Invert
            (cmd, F, Cmd::RepeatFilter),        // Last Filter (older PS)
            (cmd, Y, cmyk),                     // Proof Colors
            (cmd, R, Cmd::ToggleRulers),
            (cmd, H, Cmd::ToggleGuides),        // Extras
            (cmd, Quote, Cmd::ToggleGrid),
            (cmd, Semicolon, Cmd::ToggleGuides),
            (cmd, CloseBracket, Cmd::LayerUp),
            (cmd, OpenBracket, Cmd::LayerDown),
            (cmd, Plus, Cmd::ZoomIn),
            (cmd, Equals, Cmd::ZoomIn),
            (cmd, Minus, Cmd::ZoomOut),
            (cmd, Num0, Cmd::Fit),
            (cmd, Num1, Cmd::ActualPixels),
            (cmd, Backspace, Cmd::FillBg),
            (cmd, Delete, Cmd::FillBg),
            // Shift / Alt / plain
            (s, Backspace, Cmd::FillDialog),
            (s, F5, Cmd::FillDialog),
            (s, F6, feather),
            (a, Backspace, Cmd::FillFg),
            (a, Delete, Cmd::FillFg),
            (a, F9, Cmd::ShowPanel(crate::app::PanelId::Actions)),
            (n, F5, Cmd::ShowPanel(crate::app::PanelId::Brushes)),
            (n, F6, Cmd::ShowPanel(crate::app::PanelId::Color)),
            (n, F7, Cmd::ShowPanel(crate::app::PanelId::Layers)),
            (n, F8, Cmd::ShowPanel(crate::app::PanelId::Info)),
            (n, F12, Cmd::Revert),
            (n, Delete, Cmd::Clear),
        ];
        if ctx.wants_keyboard_input() {
            return;
        }
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            for (m, k, c) in &table {
                if i.consume_shortcut(&KeyboardShortcut::new(*m, *k)) {
                    fired.push(*c);
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
                        egui::Event::Key { key, pressed: true, .. } => Some(*key),
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
            if let Some(gi) = (0..GROUPS.len()).find(|&g| Tool::group_key(g) == Some(key)) {
                if self.tools.is_busy() || self.tools.transform.is_some() {
                    continue;
                }
                let group = GROUPS[gi];
                if mods.shift && group.contains(&self.tools.tool) && group.len() > 1 {
                    let i = group.iter().position(|t| *t == self.tools.tool).unwrap_or(0);
                    self.tools.select_tool(group[(i + 1) % group.len()]);
                } else {
                    self.tools.select_tool(self.tools.group_choice[gi]);
                }
                continue;
            }
            match key {
                Key::X => self.tools.swap_colors(),
                Key::D => self.tools.reset_colors(),
                Key::Q => self.quick_mask(ctx),
                Key::Tab => self.run(Cmd::TogglePanels, ctx),
                Key::F => self.run(Cmd::Fullscreen, ctx),
                Key::Comma | Key::Period if mods.alt => {
                    // Alt+, / Alt+. select bottom / top layer.
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        let i = if key == Key::Comma { 0 } else { tab.doc.state.layers.len().saturating_sub(1) };
                        tab.doc.select_layer(i);
                    }
                }
                Key::OpenBracket | Key::CloseBracket if mods.alt => {
                    // Alt+[ / Alt+] select layer below / above.
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        let n = tab.doc.state.layers.len();
                        let cur = tab.doc.state.active;
                        let i = if key == Key::OpenBracket { cur.saturating_sub(1) } else { (cur + 1).min(n.saturating_sub(1)) };
                        tab.doc.select_layer(i);
                    }
                }
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
                Key::Backspace => self.run(Cmd::Clear, ctx),
                Key::Slash => self.run(Cmd::ToggleLockAlpha, ctx),
                Key::Enter => {
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        self.tools.confirm(tab);
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

    /// Quick Mask (Q): converts the selection into a red overlay layer you can
    /// paint on; pressing Q again turns it back into a selection.
    fn quick_mask(&mut self, _ctx: &egui::Context) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let (w, h) = (tab.doc.width(), tab.doc.height());
        if let Some(i) = tab.doc.state.layers.iter().position(|l| l.name == crate::app::QUICK_MASK) {
            let l = tab.doc.state.layers.remove(i);
            // Red overlay alpha = unselected amount.
            let px = l.pixels;
            let m = Mask::from_fn_par(w, h, 0, |x, y| 255 - px.get(x, y)[3]);
            tab.doc.state.active = tab.doc.state.active.min(tab.doc.state.layers.len() - 1);
            tab.doc.set_selection(m.content_bounds().map(|_| m));
            tab.doc.mark_all_dirty();
            tab.doc.commit("Exit Quick Mask");
        } else {
            let sel = tab.doc.state.selection.clone();
            let px = TiledBuffer::from_fn_par(w, h, [0; 4], |x, y| {
                let s = selection::coverage(sel.as_ref(), x, y);
                [255, 0, 0, ((1.0 - s) * 128.0) as u8]
            });
            let id = tab.doc.state.alloc_id();
            tab.doc.add_layer(Layer::with_pixels(id, crate::app::QUICK_MASK, px), "Quick Mask");
            tab.doc.set_selection(None);
            self.tools.status = "Quick Mask: paint with black to add, erase to remove · Q to exit".into();
        }
    }

    pub fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let mut cmd: Option<Cmd> = None;
        let has_doc = !self.tabs.is_empty();
        let tab = self.tabs.get(self.active);
        let has_sel = tab.is_some_and(|t| t.doc.selection().is_some());
        let (can_undo, can_redo) = tab.map_or((false, false), |t| (t.doc.history().can_undo(), t.doc.history().can_redo()));
        let layer = tab.and_then(|t| t.doc.state.active_layer());
        let is_group = layer.is_some_and(|l| l.is_group());
        let has_mask = layer.is_some_and(|l| l.mask.is_some());
        let is_smart = layer.is_some_and(|l| l.smart.is_some());
        let channels: Vec<String> = tab.map(|t| t.channels.iter().map(|c| c.0.clone()).collect()).unwrap_or_default();
        let view_flags = tab.map(|t| (t.view.show_rulers, t.view.show_grid, t.view.show_guides, t.view.snap, t.view.proof));
        let undo_label = tab.filter(|_| can_undo).map(|t| format!("Undo {}", t.doc.history().current().label)).unwrap_or("Undo".into());
        let mut item = |ui: &mut egui::Ui, label: &str, sc: &str, c: Cmd, enabled: bool| {
            if ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(sc)).clicked() {
                cmd = Some(c);
                ui.close_menu();
            }
        };
        let tf = |m| Cmd::Transform(TransformModeSer::of(m));
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                item(ui, "New…", "Ctrl+N", Cmd::New, true);
                item(ui, "Open…", "Ctrl+O", Cmd::Open, true);
                item(ui, "Place Embedded…", "", Cmd::Place, has_doc);
                ui.separator();
                item(ui, "Close", "Ctrl+W", Cmd::Close, has_doc);
                item(ui, "Close All", "Alt+Ctrl+W", Cmd::CloseAll, has_doc);
                item(ui, "Save", "Ctrl+S", Cmd::Save, has_doc);
                item(ui, "Save As… (ORA / PSD)", "Shift+Ctrl+S", Cmd::SaveAs, has_doc);
                item(ui, "Revert", "", Cmd::Revert, has_doc);
                ui.separator();
                ui.menu_button("Export", |ui| {
                    item(ui, "Quick Export as PNG", "", Cmd::QuickExportPng, has_doc);
                    item(ui, "Export As…", "Alt+Shift+Ctrl+W", Cmd::Export, has_doc);
                    item(ui, "Export to PDF…", "", Cmd::ExportPdf, has_doc);
                    item(ui, "Layers to Files…", "", Cmd::ExportLayers, has_doc);
                    item(ui, "Animated GIF (frames from layers)…", "", Cmd::ExportGif, has_doc);
                });
                ui.menu_button("Automate", |ui| {
                    item(ui, "Batch…", "", Cmd::Batch, true);
                });
                ui.separator();
                item(ui, "Exit", "Ctrl+Q", Cmd::Quit, true);
            });
            ui.menu_button("Edit", |ui| {
                item(ui, &undo_label, "Ctrl+Z", Cmd::Undo, can_undo);
                item(ui, "Redo", "Shift+Ctrl+Z", Cmd::Redo, can_redo);
                ui.separator();
                item(ui, "Cut", "Ctrl+X", Cmd::Cut, has_doc);
                item(ui, "Copy", "Ctrl+C", Cmd::Copy, has_doc);
                item(ui, "Copy Merged", "Shift+Ctrl+C", Cmd::CopyMerged, has_doc);
                item(ui, "Paste", "Ctrl+V", Cmd::Paste, self.clipboard.is_some());
                item(ui, "Clear", "Del", Cmd::Clear, has_doc);
                ui.separator();
                item(ui, "Fill…", "Shift+F5", Cmd::FillDialog, has_doc);
                item(ui, "Stroke…", "", Cmd::Stroke, has_sel);
                item(ui, "Content-Aware Fill", "", Cmd::ContentAwareFill, has_sel);
                ui.separator();
                item(ui, "Free Transform", "Ctrl+T", tf(TransformMode::Free), has_doc);
                ui.menu_button("Transform", |ui| {
                    item(ui, "Scale / Rotate", "", tf(TransformMode::Free), has_doc);
                    item(ui, "Skew", "", tf(TransformMode::Skew), has_doc);
                    item(ui, "Distort", "", tf(TransformMode::Distort), has_doc);
                    item(ui, "Perspective", "", tf(TransformMode::Perspective), has_doc);
                    item(ui, "Warp", "", tf(TransformMode::Warp), has_doc);
                    ui.separator();
                    item(ui, "Numeric…", "", Cmd::TransformNumeric, has_doc);
                    for o in [Orientation::Rotate180, Orientation::Rotate90Cw, Orientation::Rotate90Ccw, Orientation::FlipHorizontal, Orientation::FlipVertical] {
                        item(ui, o.name(), "", Cmd::Orient(o), has_doc);
                    }
                });
                ui.separator();
                item(ui, "Define Brush Preset", "", Cmd::DefineBrush, has_doc);
                item(ui, "Define Pattern", "", Cmd::DefinePattern, has_doc);
            });
            ui.menu_button("Image", |ui| {
                ui.menu_button("Mode", |ui| {
                    item(ui, "Grayscale", "", Cmd::Grayscale, has_doc);
                    ui.label(egui::RichText::new("RGB Color · 8 Bits/Channel").weak());
                });
                ui.menu_button("Adjustments", |ui| {
                    for i in ADJ_MENU {
                        if i == usize::MAX {
                            ui.separator();
                        } else if i == 18 {
                            item(ui, ADJUSTMENTS[i].0, "", Cmd::Filter(27), has_doc);
                        } else {
                            let (label, sc) = ADJUSTMENTS[i];
                            item(ui, label, sc, Cmd::Adjust(i), has_doc);
                        }
                    }
                    item(ui, "HDR Toning…", "", Cmd::Filter(28), has_doc);
                });
                ui.separator();
                item(ui, "Auto Tone", "Shift+Ctrl+L", Cmd::AutoTone, has_doc);
                item(ui, "Auto Contrast", "Alt+Shift+Ctrl+L", Cmd::AutoContrast, has_doc);
                item(ui, "Auto Color", "Shift+Ctrl+B", Cmd::AutoColor, has_doc);
                ui.separator();
                item(ui, "Image Size…", "Alt+Ctrl+I", Cmd::ImageSize, has_doc);
                item(ui, "Canvas Size…", "Alt+Ctrl+C", Cmd::CanvasSize, has_doc);
                ui.menu_button("Image Rotation", |ui| {
                    for o in [Orientation::Rotate180, Orientation::Rotate90Cw, Orientation::Rotate90Ccw, Orientation::FlipHorizontal, Orientation::FlipVertical] {
                        item(ui, o.name(), "", Cmd::Orient(o), has_doc);
                    }
                });
                item(ui, "Crop", "", Cmd::CropToSelection, has_sel);
                item(ui, "Trim", "", Cmd::Trim, has_doc);
                ui.separator();
                item(ui, "Flatten Image", "", Cmd::Flatten, has_doc);
            });
            ui.menu_button("Layer", |ui| {
                ui.menu_button("New", |ui| {
                    item(ui, "Layer", "Shift+Ctrl+N", Cmd::NewLayer, has_doc);
                    item(ui, "Group", "", Cmd::NewGroup, has_doc);
                    item(ui, "Layer via Copy", "Ctrl+J", Cmd::DuplicateOrViaCopy, has_sel);
                    item(ui, "Layer via Cut", "Shift+Ctrl+J", Cmd::LayerViaCut, has_sel);
                });
                item(ui, "Duplicate Layer", "Ctrl+J", Cmd::DuplicateOrViaCopy, has_doc);
                item(ui, "Delete Layer", "", Cmd::DeleteLayer, has_doc);
                ui.separator();
                ui.menu_button("New Fill Layer", |ui| {
                    item(ui, "Solid Color (FG)", "", Cmd::NewFillLayer(0), has_doc);
                    item(ui, "Gradient (FG → BG)", "", Cmd::NewFillLayer(1), has_doc);
                    item(ui, "Pattern", "", Cmd::NewFillLayer(2), has_doc);
                });
                ui.menu_button("New Adjustment Layer", |ui| {
                    for i in ADJ_MENU {
                        if i == usize::MAX {
                            ui.separator();
                        } else if i != 18 {
                            item(ui, ADJUSTMENTS[i].0.trim_end_matches('…'), "", Cmd::NewAdjLayer(i), has_doc);
                        }
                    }
                });
                ui.separator();
                ui.menu_button("Layer Style", |ui| {
                    item(ui, "Blending Options / Effects…", "", Cmd::LayerStyle, has_doc);
                    item(ui, "Copy Layer Style", "", Cmd::CopyStyle, has_doc);
                    item(ui, "Paste Layer Style", "", Cmd::PasteStyle, self.style_clipboard.is_some());
                    item(ui, "Clear Layer Style", "", Cmd::ClearStyle, has_doc);
                });
                ui.menu_button("Layer Mask", |ui| {
                    item(ui, "Reveal All", "", Cmd::AddMask, has_doc && !has_mask);
                    item(ui, "Hide All", "", Cmd::HideAllMask, has_doc && !has_mask);
                    item(ui, "Reveal Selection", "", Cmd::MaskFromSelection, has_sel && !has_mask);
                    item(ui, "Invert", "", Cmd::InvertMask, has_mask);
                    item(ui, "Delete", "", Cmd::DeleteMask, has_mask);
                    item(ui, "Apply", "", Cmd::ApplyMask, has_mask);
                });
                item(ui, "Create / Release Clipping Mask", "Alt+Ctrl+G", Cmd::ToggleClip, has_doc);
                ui.separator();
                ui.menu_button("Smart Objects", |ui| {
                    item(ui, "Convert to Smart Object", "", Cmd::ConvertSmart, has_doc && !is_smart);
                    item(ui, "Rasterize", "", Cmd::Rasterize, is_smart);
                });
                item(ui, "Rasterize Layer", "", Cmd::Rasterize, has_doc);
                ui.separator();
                item(ui, "Group Layers", "Ctrl+G", Cmd::GroupLayers, has_doc);
                item(ui, "Ungroup Layers", "Shift+Ctrl+G", Cmd::Ungroup, is_group);
                ui.menu_button("Arrange", |ui| {
                    item(ui, "Bring to Front", "Shift+Ctrl+]", Cmd::LayerTop, has_doc);
                    item(ui, "Bring Forward", "Ctrl+]", Cmd::LayerUp, has_doc);
                    item(ui, "Send Backward", "Ctrl+[", Cmd::LayerDown, has_doc);
                    item(ui, "Send to Back", "Shift+Ctrl+[", Cmd::LayerBottom, has_doc);
                });
                ui.menu_button(if has_sel { "Align Layer to Selection" } else { "Align to Canvas" }, |ui| {
                    for (n, k) in [
                        ("Left Edges", AlignKind::Left),
                        ("Horizontal Centers", AlignKind::HCenter),
                        ("Right Edges", AlignKind::Right),
                        ("Top Edges", AlignKind::Top),
                        ("Vertical Centers", AlignKind::VCenter),
                        ("Bottom Edges", AlignKind::Bottom),
                    ] {
                        item(ui, n, "", Cmd::Align(k), has_doc);
                    }
                });
                item(ui, "Lock Transparent Pixels", "/", Cmd::ToggleLockAlpha, has_doc);
                ui.separator();
                item(ui, if is_group { "Merge Group" } else { "Merge Down" }, "Ctrl+E", Cmd::MergeDown, has_doc);
                item(ui, "Merge Visible", "Shift+Ctrl+E", Cmd::MergeVisible, has_doc);
                item(ui, "Stamp Visible", "Alt+Shift+Ctrl+E", Cmd::StampVisible, has_doc);
                item(ui, "Flatten Image", "", Cmd::Flatten, has_doc);
            });
            ui.menu_button("Select", |ui| {
                item(ui, "All", "Ctrl+A", Cmd::SelectAll, has_doc);
                item(ui, "Deselect", "Ctrl+D", Cmd::Deselect, has_sel);
                item(ui, "Reselect", "Shift+Ctrl+D", Cmd::Reselect, has_doc);
                item(ui, "Inverse", "Shift+Ctrl+I", Cmd::Inverse, has_sel);
                ui.separator();
                item(ui, "Color Range…", "", Cmd::ColorRange, has_doc);
                item(ui, "Focus Area…", "", Cmd::FocusArea, has_doc);
                item(ui, "Load Layer Transparency", "Ctrl+click thumb", Cmd::SelectionFromAlpha, has_doc);
                ui.menu_button("Modify", |ui| {
                    for (n, k, sc) in [
                        ("Border…", SelModifyKind::Border, ""),
                        ("Expand…", SelModifyKind::Expand, ""),
                        ("Contract…", SelModifyKind::Contract, ""),
                        ("Feather…", SelModifyKind::Feather, "Shift+F6"),
                    ] {
                        item(ui, n, sc, Cmd::SelModify(SelModifyKindSer::of(k)), has_sel);
                    }
                });
                ui.separator();
                item(ui, "Save Selection", "", Cmd::SaveSelection, has_sel);
                ui.menu_button("Load Selection", |ui| {
                    if channels.is_empty() {
                        ui.label("No saved selections");
                    }
                    for (i, n) in channels.iter().enumerate() {
                        item(ui, n, "", Cmd::LoadSelection(i), true);
                    }
                });
                item(ui, "Make Work Path", "", Cmd::MakeWorkPath, has_sel);
                item(ui, "Quick Mask Mode", "Q", Cmd::Deselect, false);
            });
            ui.menu_button("Filter", |ui| {
                let last = self.last_filter.as_ref().map(|f| format!("Last Filter ({})", f.name())).unwrap_or("Last Filter".into());
                item(ui, &last, "Ctrl+F", Cmd::RepeatFilter, has_doc && self.last_filter.is_some());
                ui.separator();
                item(ui, "Convert for Smart Filters", "", Cmd::ConvertSmart, has_doc && !is_smart);
                item(ui, "Liquify", "Shift+Ctrl+X", Cmd::Liquify, has_doc);
                ui.separator();
                let mut groups: Vec<&str> = Vec::new();
                for (g, _) in FILTERS {
                    if !groups.contains(&g) {
                        groups.push(g);
                    }
                }
                groups.sort_unstable();
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
                ui.menu_button("Proof Colors", |ui| {
                    for p in Proof::ALL {
                        let on = view_flags.is_some_and(|f| f.4 == p);
                        item(ui, &format!("{}{}", if on { "✔ " } else { "" }, p.name()), "", Cmd::Proof(ProofSer::of(p)), has_doc);
                    }
                });
                ui.separator();
                item(ui, "Zoom In", "Ctrl++", Cmd::ZoomIn, has_doc);
                item(ui, "Zoom Out", "Ctrl+-", Cmd::ZoomOut, has_doc);
                item(ui, "Fit on Screen", "Ctrl+0", Cmd::Fit, has_doc);
                item(ui, "100%", "Ctrl+1", Cmd::ActualPixels, has_doc);
                ui.separator();
                let check = |b: bool, n: &str| format!("{}{n}", if b { "✔ " } else { "    " });
                let (r, g, gu, sn) = view_flags.map_or((false, false, false, false), |f| (f.0, f.1, f.2, f.3));
                item(ui, &check(r, "Rulers"), "Ctrl+R", Cmd::ToggleRulers, has_doc);
                item(ui, &check(g, "Grid"), "Ctrl+'", Cmd::ToggleGrid, has_doc);
                item(ui, &check(gu, "Guides"), "Ctrl+;", Cmd::ToggleGuides, has_doc);
                item(ui, &check(sn, "Snap"), "Shift+Ctrl+;", Cmd::ToggleSnap, has_doc);
                item(ui, "New Guide…", "", Cmd::NewGuide, has_doc);
                item(ui, "Clear Guides", "", Cmd::ClearGuides, has_doc);
                ui.separator();
                item(ui, "Full Screen", "F", Cmd::Fullscreen, true);
                item(ui, "Show/Hide Panels", "Tab", Cmd::TogglePanels, true);
                item(ui, if self.light_theme { "Dark Interface" } else { "Light Interface" }, "", Cmd::ToggleTheme, true);
            });
            ui.menu_button("Window", |ui| {
                for (name, id, sc) in [
                    ("Actions", crate::app::PanelId::Actions, "Alt+F9"),
                    ("Adjustments", crate::app::PanelId::Adjustments, ""),
                    ("Brushes", crate::app::PanelId::Brushes, "F5"),
                    ("Channels", crate::app::PanelId::Channels, ""),
                    ("Color", crate::app::PanelId::Color, "F6"),
                    ("Gradients", crate::app::PanelId::Gradients, ""),
                    ("Histogram", crate::app::PanelId::Histogram, ""),
                    ("History", crate::app::PanelId::History, ""),
                    ("Info", crate::app::PanelId::Info, "F8"),
                    ("Layers", crate::app::PanelId::Layers, "F7"),
                    ("Navigator", crate::app::PanelId::Navigator, ""),
                    ("Paths", crate::app::PanelId::Paths, ""),
                    ("Patterns", crate::app::PanelId::Patterns, ""),
                    ("Properties", crate::app::PanelId::Properties, ""),
                    ("Swatches", crate::app::PanelId::Swatches, ""),
                ] {
                    let on = self.panels.is_front(id);
                    item(ui, &format!("{}{name}", if on { "✔ " } else { "    " }), sc, Cmd::ShowPanel(id), true);
                }
                ui.separator();
                item(ui, "Add Font…", "", Cmd::AddFont, true);
            });
            ui.menu_button("Help", |ui| {
                item(ui, "Keyboard Shortcuts…", "Alt+Shift+Ctrl+K", Cmd::Shortcuts, true);
                ui.separator();
                item(ui, "Check for Updates…", "", Cmd::CheckUpdates, true);
                item(ui, "Update from Source (GitHub main)…", "", Cmd::UpdateFromSource, true);
                ui.separator();
                item(ui, "About Forge Design", "", Cmd::About, true);
            });
        });
        if let Some(c) = cmd {
            let ctx = ui.ctx().clone();
            self.run(c, &ctx);
        }
    }
}

/// Re-renders an edited type layer.
pub fn rerender_text(tab: &mut DocTab, index: usize, fonts: &[crate::text::FontEntry]) {
    let (w, h) = (tab.doc.width(), tab.doc.height());
    let Some(l) = tab.doc.state.layers.get(index) else { return };
    let Some(data) = l.text.clone() else { return };
    let entry = fonts
        .iter()
        .find(|f| f.path.as_ref().map(|p| p.to_string_lossy().to_string()) == data.font_path)
        .or(fonts.first());
    if let Some(font) = entry.and_then(crate::text::load_font) {
        let px: TiledBuffer<Rgba8> = crate::text::render_text_data(&font, &data, w, h);
        tab.doc.state.layers[index].pixels = px;
        tab.doc.mark_all_dirty();
    }
}
