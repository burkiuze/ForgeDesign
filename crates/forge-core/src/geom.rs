//! Small geometry primitives.

use serde::{Deserialize, Serialize};

/// A point in image space (pixels, sub-pixel precision).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, o: Point) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }

    pub fn lerp(self, o: Point, t: f32) -> Point {
        Point::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
}

/// Integer rectangle, `x1`/`y1` exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn from_size(w: u32, h: u32) -> Self {
        Self::new(0, 0, w as i32, h as i32)
    }

    /// Normalised rectangle spanned by two (possibly unordered) points.
    pub fn from_points(a: Point, b: Point) -> Self {
        Self::new(
            a.x.min(b.x).floor() as i32,
            a.y.min(b.y).floor() as i32,
            a.x.max(b.x).ceil() as i32,
            a.y.max(b.y).ceil() as i32,
        )
    }

    /// Bounding box of a disc, padded by one pixel for antialiasing.
    pub fn around(c: Point, radius: f32) -> Self {
        Self::new(
            (c.x - radius - 1.0).floor() as i32,
            (c.y - radius - 1.0).floor() as i32,
            (c.x + radius + 1.0).ceil() as i32 + 1,
            (c.y + radius + 1.0).ceil() as i32 + 1,
        )
    }

    pub fn width(&self) -> i32 {
        (self.x1 - self.x0).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.y1 - self.y0).max(0)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }

    pub fn intersect(&self, o: &IRect) -> IRect {
        IRect::new(
            self.x0.max(o.x0),
            self.y0.max(o.y0),
            self.x1.min(o.x1),
            self.y1.min(o.y1),
        )
    }

    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        IRect::new(
            self.x0.min(o.x0),
            self.y0.min(o.y0),
            self.x1.max(o.x1),
            self.y1.max(o.y1),
        )
    }

    pub fn expand(&self, n: i32) -> IRect {
        IRect::new(self.x0 - n, self.y0 - n, self.x1 + n, self.y1 + n)
    }

    pub fn translate(&self, dx: i32, dy: i32) -> IRect {
        IRect::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }

    pub fn clamp_to(&self, w: u32, h: u32) -> IRect {
        self.intersect(&IRect::from_size(w, h))
    }
}

/// Union of two optional dirty rectangles.
pub fn union_opt(a: Option<IRect>, b: Option<IRect>) -> Option<IRect> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.union(&b)),
        (a, None) => a,
        (None, b) => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ops() {
        let a = IRect::new(0, 0, 10, 10);
        let b = IRect::new(5, 5, 20, 20);
        assert_eq!(a.intersect(&b), IRect::new(5, 5, 10, 10));
        assert_eq!(a.union(&b), IRect::new(0, 0, 20, 20));
        assert!(IRect::new(3, 3, 3, 9).is_empty());
        assert_eq!(
            IRect::from_points(Point::new(9.5, 2.0), Point::new(1.2, 7.7)),
            IRect::new(1, 2, 10, 8)
        );
    }
}
