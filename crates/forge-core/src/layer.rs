//! Raster layers.

use crate::blend::BlendMode;
use crate::buffer::TiledBuffer;
use crate::color::Rgba8;

pub type LayerId = u64;

/// A layer mask: 255 reveals, 0 hides. Unallocated tiles reveal everything.
pub type LayerMask = TiledBuffer<u8>;

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
        }
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
    }
}
