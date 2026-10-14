//! Raster layers.

use crate::blend::BlendMode;
use crate::buffer::TiledBuffer;
use serde::{Deserialize, Serialize};

use crate::adjust::Adjustment;
use crate::color::{Color, Rgba8};
use crate::filters::Filter;
use crate::styles::LayerStyle;

pub type LayerId = u64;

/// A layer mask: 255 reveals, 0 hides. Unallocated tiles reveal everything.
pub type LayerMask = TiledBuffer<u8>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayerKind {
    #[default]
    Pixel,
    /// A folder; its children have `parent == Some(group.id)`.
    Group,
    /// Non-destructive adjustment layer (see [`Layer::adjustment`]).
    Adjustment,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Editable text of a type layer (rasterised by the application).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextData {
    pub text: String,
    /// Font file path; `None` = built-in font.
    pub font_path: Option<String>,
    pub font_name: String,
    pub size: f32,
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Extra letter spacing in 1/1000 em (Photoshop tracking units).
    pub tracking: f32,
    /// Line spacing multiplier.
    pub leading: f32,
    pub align: TextAlign,
    pub vertical: bool,
    /// Paragraph text box width (wraps words); `None` = point text.
    pub box_width: Option<f32>,
    /// Arc warp, -100..100.
    pub warp: f32,
    pub x: f32,
    pub y: f32,
    pub antialias: bool,
}

/// Smart Object: the original pixels plus a non-destructive transform and
/// smart filters. `Layer::pixels` holds the rendered result.
#[derive(Clone, Debug, PartialEq)]
pub struct SmartObject {
    pub source: TiledBuffer<Rgba8>,
    /// Where the source bounds' corners (TL, TR, BR, BL) land.
    pub quad: [(f32, f32); 4],
    pub filters: Vec<SmartFilter>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SmartFilter {
    pub filter: Filter,
    pub enabled: bool,
}

impl SmartObject {
    pub fn new(source: TiledBuffer<Rgba8>) -> Self {
        let b = source.content_bounds().unwrap_or(source.bounds());
        let quad = [(b.x0 as f32, b.y0 as f32), (b.x1 as f32, b.y0 as f32), (b.x1 as f32, b.y1 as f32), (b.x0 as f32, b.y1 as f32)];
        Self { source, quad, filters: Vec::new() }
    }

    /// Re-renders the layer pixels from the source (quality never degrades).
    pub fn render(&self) -> TiledBuffer<Rgba8> {
        let b = self.source.content_bounds().unwrap_or(self.source.bounds());
        let ident = [(b.x0 as f32, b.y0 as f32), (b.x1 as f32, b.y0 as f32), (b.x1 as f32, b.y1 as f32), (b.x0 as f32, b.y1 as f32)];
        let mut px = if self.quad == ident {
            self.source.clone()
        } else {
            crate::transform::transform_pixels_quad(&self.source, None, self.quad, crate::transform::Resample::Bilinear).0
        };
        for f in self.filters.iter().filter(|f| f.enabled) {
            let base = px.clone();
            crate::filters::apply(&mut px, &base, None, &f.filter);
        }
        px
    }
}

/// Photoshop's layer colour labels.
pub const LABEL_COLORS: [(&str, [u8; 3]); 8] = [
    ("None", [0, 0, 0]),
    ("Red", [230, 80, 80]),
    ("Orange", [240, 150, 60]),
    ("Yellow", [235, 210, 70]),
    ("Green", [110, 200, 100]),
    ("Blue", [90, 150, 235]),
    ("Violet", [170, 120, 230]),
    ("Gray", [150, 150, 150]),
];

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    /// Layer opacity, `0.0..=1.0`.
    pub opacity: f32,
    pub blend: BlendMode,
    /// Painting only affects already-opaque pixels ("Lock transparent pixels").
    pub lock_alpha: bool,
    /// Fully locked: tools refuse to modify the layer.
    pub locked: bool,
    pub pixels: TiledBuffer<Rgba8>,
    pub mask: Option<LayerMask>,
    pub mask_enabled: bool,
    /// Fill opacity: like opacity but does not affect layer styles.
    pub fill: f32,
    /// Clipping mask: only visible where the layer below is opaque.
    pub clipped: bool,
    pub style: Option<LayerStyle>,
    pub kind: LayerKind,
    /// Containing group, if any.
    pub parent: Option<LayerId>,
    /// Group is collapsed in the Layers panel.
    pub collapsed: bool,
    /// Index into [`LABEL_COLORS`].
    pub label: u8,
    /// For [`LayerKind::Adjustment`] layers.
    pub adjustment: Option<Adjustment>,
    /// Type layers keep their text editable.
    pub text: Option<TextData>,
    pub smart: Option<Box<SmartObject>>,
}

impl Layer {
    pub fn new(id: LayerId, name: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            lock_alpha: false,
            locked: false,
            pixels: TiledBuffer::new(width, height),
            mask: None,
            mask_enabled: true,
            fill: 1.0,
            clipped: false,
            style: None,
            kind: LayerKind::Pixel,
            parent: None,
            collapsed: false,
            label: 0,
            adjustment: None,
            text: None,
            smart: None,
        }
    }

    pub fn new_adjustment(id: LayerId, adj: Adjustment, width: u32, height: u32) -> Self {
        let mut l = Self::new(id, adj.name(), width, height);
        l.kind = LayerKind::Adjustment;
        l.adjustment = Some(adj);
        l.add_mask();
        l
    }

    pub fn is_adjustment(&self) -> bool {
        self.kind == LayerKind::Adjustment
    }

    /// Pixel layers that tools may paint on (not groups/adjustments).
    pub fn has_pixels(&self) -> bool {
        self.kind == LayerKind::Pixel
    }

    pub fn new_group(id: LayerId, name: impl Into<String>, width: u32, height: u32) -> Self {
        let mut l = Self::new(id, name, width, height);
        l.kind = LayerKind::Group;
        l
    }

    pub fn is_group(&self) -> bool {
        self.kind == LayerKind::Group
    }

    pub fn with_pixels(id: LayerId, name: impl Into<String>, pixels: TiledBuffer<Rgba8>) -> Self {
        let mut l = Self::new(id, name, pixels.width(), pixels.height());
        l.pixels = pixels;
        l
    }

    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    pub fn height(&self) -> u32 {
        self.pixels.height()
    }

    /// Adds a white (reveal-all) mask.
    pub fn add_mask(&mut self) {
        if self.mask.is_none() {
            self.mask = Some(TiledBuffer::new_filled(self.width(), self.height(), 255));
            self.mask_enabled = true;
        }
    }

    /// The mask that should influence compositing, if any.
    pub fn active_mask(&self) -> Option<&LayerMask> {
        self.mask.as_ref().filter(|_| self.mask_enabled)
    }

    /// Bakes the mask into the pixel alpha and removes it.
    pub fn apply_mask(&mut self) {
        if let Some(mask) = self.mask.take() {
            self.pixels.par_map_all(|x, y, mut p| {
                let m = mask.get(x, y) as u32;
                p[3] = ((p[3] as u32 * m + 127) / 255) as u8;
                p
            });
            self.pixels.optimize();
        }
    }

    /// Whether the two layers have identical non-pixel properties.
    pub fn same_props(&self, o: &Layer) -> bool {
        self.id == o.id
            && self.visible == o.visible
            && self.opacity == o.opacity
            && self.blend == o.blend
            && self.mask.is_some() == o.mask.is_some()
            && self.mask_enabled == o.mask_enabled
            && self.fill == o.fill
            && self.clipped == o.clipped
            && self.style == o.style
            && self.kind == o.kind
            && self.parent == o.parent
            && self.adjustment == o.adjustment
            && self.label == o.label
    }
}
