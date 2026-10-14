//! Sparse, tiled, copy-on-write pixel storage.
//!
//! A [`TiledBuffer`] is a grid of `TILE_SIZE × TILE_SIZE` tiles. Each tile is
//! either absent (every pixel equals the buffer's fill value) or an
//! `Arc<Vec<P>>`. Cloning the buffer only bumps reference counts; writing to
//! a shared tile clones just that tile (`Arc::make_mut`). History snapshots,
//! stroke base copies and filter previews are therefore cheap.

use std::sync::Arc;

use rayon::prelude::*;

use crate::geom::IRect;

pub const TILE_SHIFT: u32 = 6;
pub const TILE_SIZE: u32 = 1 << TILE_SHIFT;
const TILE_MASK: u32 = TILE_SIZE - 1;
pub const TILE_PIXELS: usize = (TILE_SIZE * TILE_SIZE) as usize;

/// Anything that can be stored in a tiled buffer.
pub trait Pixel: Copy + Default + PartialEq + Send + Sync + 'static {}
impl<T: Copy + Default + PartialEq + Send + Sync + 'static> Pixel for T {}

type Tile<P> = Arc<Vec<P>>;

#[derive(Clone, Debug)]
pub struct TiledBuffer<P: Pixel> {
    width: u32,
    height: u32,
    tiles_x: u32,
    tiles_y: u32,
    fill: P,
    tiles: Vec<Option<Tile<P>>>,
}

impl<P: Pixel> TiledBuffer<P> {
    /// Creates an empty buffer where every pixel is `P::default()`.
    pub fn new(width: u32, height: u32) -> Self {
        Self::new_filled(width, height, P::default())
    }

    /// Creates a buffer where every pixel is `fill`. No memory is allocated
    /// for pixel data until something is written.
    pub fn new_filled(width: u32, height: u32, fill: P) -> Self {
        let tiles_x = width.div_ceil(TILE_SIZE).max(1);
        let tiles_y = height.div_ceil(TILE_SIZE).max(1);
        Self {
            width,
            height,
            tiles_x,
            tiles_y,
            fill,
            tiles: vec![None; (tiles_x * tiles_y) as usize],
        }
    }

    /// Creates a buffer with every pixel materialised to `value`
    /// (unlike `new_filled`, the fill value stays `P::default()`).
    pub fn solid(width: u32, height: u32, value: P) -> Self {
        let mut b = Self::new(width, height);
        if value != P::default() {
            let tile: Tile<P> = Arc::new(vec![value; TILE_PIXELS]);
            for t in b.tiles.iter_mut() {
                *t = Some(tile.clone());
            }
        }
        b
    }

    pub fn from_dense(width: u32, height: u32, data: &[P]) -> Self {
        assert_eq!(data.len(), (width * height) as usize);
        let mut b = Self::new(width, height);
        let fill = b.fill;
        let tiles_x = b.tiles_x;
        b.tiles.par_iter_mut().enumerate().for_each(|(i, slot)| {
            let tx = i as u32 % tiles_x;
            let ty = i as u32 / tiles_x;
            let x0 = tx * TILE_SIZE;
            let y0 = ty * TILE_SIZE;
            let mut tile = vec![fill; TILE_PIXELS];
            let mut any = false;
            for ly in 0..TILE_SIZE.min(height - y0) {
                let row = ((y0 + ly) * width + x0) as usize;
                let n = TILE_SIZE.min(width - x0) as usize;
                let dst = &mut tile[(ly * TILE_SIZE) as usize..][..n];
                dst.copy_from_slice(&data[row..row + n]);
                any |= dst.iter().any(|p| *p != fill);
            }
            if any {
                *slot = Some(Arc::new(tile));
            }
        });
        b
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn tiles_x(&self) -> u32 {
        self.tiles_x
    }

    pub fn tiles_y(&self) -> u32 {
        self.tiles_y
    }

    pub fn fill_value(&self) -> P {
        self.fill
    }

    pub fn bounds(&self) -> IRect {
        IRect::from_size(self.width, self.height)
    }

    #[inline]
    fn tile_index(&self, tx: u32, ty: u32) -> usize {
        (ty * self.tiles_x + tx) as usize
    }

    /// Pixel value; out-of-bounds coordinates return the fill value.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> P {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return self.fill;
        }
        let (x, y) = (x as u32, y as u32);
        match &self.tiles[self.tile_index(x >> TILE_SHIFT, y >> TILE_SHIFT)] {
            Some(t) => t[((y & TILE_MASK) * TILE_SIZE + (x & TILE_MASK)) as usize],
            None => self.fill,
        }
    }

    /// Sets a pixel; out-of-bounds writes are ignored.
    #[inline]
    pub fn set(&mut self, x: i32, y: i32, p: P) {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return;
        }
        let (x, y) = (x as u32, y as u32);
        let idx = self.tile_index(x >> TILE_SHIFT, y >> TILE_SHIFT);
        let fill = self.fill;
        if self.tiles[idx].is_none() && p == fill {
            return;
        }
        let tile = self.tiles[idx].get_or_insert_with(|| Arc::new(vec![fill; TILE_PIXELS]));
        Arc::make_mut(tile)[((y & TILE_MASK) * TILE_SIZE + (x & TILE_MASK)) as usize] = p;
    }

    /// Read-only access to a tile's pixel data (`None` = uniform fill).
    pub fn tile(&self, tx: u32, ty: u32) -> Option<&[P]> {
        self.tiles[self.tile_index(tx, ty)].as_deref().map(|v| v.as_slice())
    }

    /// Raw `Arc` access, used for cheap change detection (`Arc::ptr_eq`).
    pub fn tile_arc(&self, tx: u32, ty: u32) -> Option<&Arc<Vec<P>>> {
        self.tiles[self.tile_index(tx, ty)].as_ref()
    }

    /// Mutable tile data, allocating/un-sharing the tile as needed.
    pub fn tile_mut(&mut self, tx: u32, ty: u32) -> &mut [P] {
        let idx = self.tile_index(tx, ty);
        let fill = self.fill;
        let tile = self.tiles[idx].get_or_insert_with(|| Arc::new(vec![fill; TILE_PIXELS]));
        Arc::make_mut(tile).as_mut_slice()
    }

    /// Pixel rectangle covered by a tile, clipped to the image.
    pub fn tile_rect(&self, tx: u32, ty: u32) -> IRect {
        let x0 = (tx * TILE_SIZE) as i32;
        let y0 = (ty * TILE_SIZE) as i32;
        IRect::new(x0, y0, x0 + TILE_SIZE as i32, y0 + TILE_SIZE as i32)
            .clamp_to(self.width, self.height)
    }

    /// Range of tile coordinates touching `rect` (clipped to the image).
    pub fn tile_range(&self, rect: IRect) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
        let r = rect.clamp_to(self.width, self.height);
        if r.is_empty() {
            return (0..0, 0..0);
        }
        (
            (r.x0 as u32 >> TILE_SHIFT)..((r.x1 as u32 - 1) >> TILE_SHIFT) + 1,
            (r.y0 as u32 >> TILE_SHIFT)..((r.y1 as u32 - 1) >> TILE_SHIFT) + 1,
        )
    }

    pub fn is_tile_allocated(&self, tx: u32, ty: u32) -> bool {
        self.tiles[self.tile_index(tx, ty)].is_some()
    }

    pub fn clear_tile(&mut self, tx: u32, ty: u32) {
        let idx = self.tile_index(tx, ty);
        self.tiles[idx] = None;
    }

    /// Resets the whole buffer to its fill value.
    pub fn clear(&mut self) {
        self.tiles.iter_mut().for_each(|t| *t = None);
    }

    /// Number of materialised tiles (memory-usage metric).
    pub fn allocated_tiles(&self) -> usize {
        self.tiles.iter().filter(|t| t.is_some()).count()
    }

    /// Calls `f(x, y, &mut pixel)` for every pixel in `rect`, sequentially.
    pub fn for_each_in_rect_mut(&mut self, rect: IRect, mut f: impl FnMut(i32, i32, &mut P)) {
        let r = rect.clamp_to(self.width, self.height);
        let (txs, tys) = self.tile_range(r);
        for ty in tys {
            for tx in txs.clone() {
                let tr = self.tile_rect(tx, ty).intersect(&r);
                let data = self.tile_mut(tx, ty);
                let (bx, by) = ((tx * TILE_SIZE) as i32, (ty * TILE_SIZE) as i32);
                for y in tr.y0..tr.y1 {
                    let row = ((y - by) as u32 * TILE_SIZE) as usize;
                    for x in tr.x0..tr.x1 {
                        f(x, y, &mut data[row + (x - bx) as usize]);
                    }
                }
            }
        }
    }

    /// Parallel per-pixel update of every pixel in `rect` (tile-parallel).
    pub fn par_for_each_in_rect_mut(&mut self, rect: IRect, f: impl Fn(i32, i32, &mut P) + Sync) {
        let r = rect.clamp_to(self.width, self.height);
        if r.is_empty() {
            return;
        }
        let (txs, tys) = self.tile_range(r);
        let tiles_x = self.tiles_x;
        let fill = self.fill;
        self.tiles.par_iter_mut().enumerate().for_each(|(i, slot)| {
            let tx = i as u32 % tiles_x;
            let ty = i as u32 / tiles_x;
            if !txs.contains(&tx) || !tys.contains(&ty) {
                return;
            }
            let (bx, by) = ((tx * TILE_SIZE) as i32, (ty * TILE_SIZE) as i32);
            let tr = IRect::new(bx, by, bx + TILE_SIZE as i32, by + TILE_SIZE as i32).intersect(&r);
            let data = Arc::make_mut(slot.get_or_insert_with(|| Arc::new(vec![fill; TILE_PIXELS])));
            for y in tr.y0..tr.y1 {
                let row = ((y - by) as u32 * TILE_SIZE) as usize;
                for x in tr.x0..tr.x1 {
                    f(x, y, &mut data[row + (x - bx) as usize]);
                }
            }
        });
    }

    /// Parallel `map` over every pixel of the image.
    pub fn par_map_all(&mut self, f: impl Fn(i32, i32, P) -> P + Sync) {
        self.par_for_each_in_rect_mut(self.bounds(), |x, y, p| *p = f(x, y, *p));
    }

    /// Copies the image into a row-major `Vec`.
    pub fn to_dense(&self) -> Vec<P> {
        self.region_to_dense(self.bounds())
    }

    /// Copies `rect` (may extend beyond the image; outside reads fill) into a row-major `Vec`.
    pub fn region_to_dense(&self, rect: IRect) -> Vec<P> {
        let w = rect.width() as usize;
        let h = rect.height() as usize;
        let mut out = vec![self.fill; w * h];
        if w == 0 {
            return out;
        }
        out.par_chunks_mut(w).enumerate().for_each(|(row, dst)| {
            let y = rect.y0 + row as i32;
            if y < 0 || y as u32 >= self.height {
                return;
            }
            let ty = y as u32 >> TILE_SHIFT;
            let ly = (y as u32 & TILE_MASK) * TILE_SIZE;
            let mut x = rect.x0.max(0);
            let xend = rect.x1.min(self.width as i32);
            while x < xend {
                let tx = x as u32 >> TILE_SHIFT;
                let tile_end = (((tx + 1) * TILE_SIZE) as i32).min(xend);
                if let Some(t) = &self.tiles[self.tile_index(tx, ty)] {
                    let src = &t[(ly + (x as u32 & TILE_MASK)) as usize..][..(tile_end - x) as usize];
                    dst[(x - rect.x0) as usize..(tile_end - rect.x0) as usize].copy_from_slice(src);
                }
                x = tile_end;
            }
        });
        out
    }

    /// Writes a dense row-major block at `rect` (clipped to the image).
    pub fn write_dense(&mut self, rect: IRect, data: &[P]) {
        let w = rect.width();
        assert_eq!(data.len(), (w * rect.height()) as usize);
        let src_rect = rect;
        self.par_for_each_in_rect_mut(rect, |x, y, p| {
            *p = data[((y - src_rect.y0) * w + (x - src_rect.x0)) as usize];
        });
    }

    /// Drops tiles whose pixels all equal the fill value (re-sparsifies).
    pub fn optimize(&mut self) {
        let fill = self.fill;
        self.tiles.par_iter_mut().for_each(|slot| {
            if let Some(t) = slot {
                if t.iter().all(|p| *p == fill) {
                    *slot = None;
                }
            }
        });
    }

    /// Smallest rectangle containing every non-fill pixel.
    pub fn content_bounds(&self) -> Option<IRect> {
        let fill = self.fill;
        let rects: Vec<IRect> = (0..self.tiles.len())
            .into_par_iter()
            .filter_map(|i| {
                let t = self.tiles[i].as_ref()?;
                let tx = i as u32 % self.tiles_x;
                let ty = i as u32 / self.tiles_x;
                let tr = self.tile_rect(tx, ty);
                let mut r: Option<IRect> = None;
                for y in tr.y0..tr.y1 {
                    let row = ((y - tr.y0) as u32 * TILE_SIZE) as usize;
                    for x in tr.x0..tr.x1 {
                        if t[row + (x - tr.x0) as usize] != fill {
                            let px = IRect::new(x, y, x + 1, y + 1);
                            r = Some(r.map_or(px, |r| r.union(&px)));
                        }
                    }
                }
                r
            })
            .collect();
        rects.into_iter().reduce(|a, b| a.union(&b))
    }

    /// True when the two buffers share the identical tile allocation at `(tx, ty)`.
    pub fn same_tile(&self, other: &Self, tx: u32, ty: u32) -> bool {
        match (self.tile_arc(tx, ty), other.tile_arc(tx, ty)) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Bounding box of tiles that differ (by allocation) between two buffers
    /// of identical dimensions. O(tiles), no pixel comparison.
    pub fn diff_rect(&self, other: &Self) -> Option<IRect> {
        if self.width != other.width || self.height != other.height {
            return Some(self.bounds());
        }
        let mut r: Option<IRect> = None;
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                if !self.same_tile(other, tx, ty) {
                    let tr = self.tile_rect(tx, ty);
                    r = Some(r.map_or(tr, |r| r.union(&tr)));
                }
            }
        }
        r
    }

    /// Creates a buffer of another size where each pixel is computed by `f(x, y)`.
    pub fn from_fn_par(width: u32, height: u32, fill: P, f: impl Fn(i32, i32) -> P + Sync) -> Self {
        let mut b = Self::new_filled(width, height, fill);
        b.par_for_each_in_rect_mut(IRect::from_size(width, height), |x, y, p| *p = f(x, y));
        b.optimize();
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_and_cow() {
        let mut a: TiledBuffer<u8> = TiledBuffer::new(200, 100);
        assert_eq!(a.allocated_tiles(), 0);
        a.set(70, 10, 5);
        assert_eq!(a.allocated_tiles(), 1);
        assert_eq!(a.get(70, 10), 5);
        assert_eq!(a.get(-1, 10), 0);

        let b = a.clone();
        a.set(70, 10, 9);
        assert_eq!(b.get(70, 10), 5, "clone must not see later writes");
        assert_eq!(a.get(70, 10), 9);
        assert_eq!(a.diff_rect(&b), Some(IRect::new(64, 0, 128, 64)));
    }

    #[test]
    fn dense_roundtrip() {
        let w = 130;
        let h = 70;
        let data: Vec<u8> = (0..w * h).map(|i| (i % 251) as u8).collect();
        let b = TiledBuffer::from_dense(w, h, &data);
        assert_eq!(b.to_dense(), data);
        let r = IRect::new(60, 60, 70, 75);
        let region = b.region_to_dense(r);
        assert_eq!(region.len(), 150);
        assert_eq!(region[0], data[(60 * w + 60) as usize]);
        assert_eq!(region[149], 0, "outside the image reads fill");
    }

    #[test]
    fn rect_iteration() {
        let mut b: TiledBuffer<u32> = TiledBuffer::new(300, 300);
        b.par_for_each_in_rect_mut(IRect::new(50, 50, 150, 70), |x, y, p| *p = (x + y) as u32);
        assert_eq!(b.get(100, 60), 160);
        assert_eq!(b.get(150, 60), 0);
        assert_eq!(b.content_bounds(), Some(IRect::new(50, 50, 150, 70)));
        let mut n = 0;
        b.for_each_in_rect_mut(IRect::new(0, 0, 300, 300), |_, _, p| n += (*p > 0) as u32);
        assert_eq!(n, 100 * 20);
    }
}
