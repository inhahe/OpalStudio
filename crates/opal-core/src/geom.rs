//! Integer rectangles in canvas pixels.

/// A rectangle of canvas pixels, `x0 <= x < x1`, `y0 <= y < y1`. Coordinates are 64-bit
/// because layers are unbounded and may extend far past the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IRect {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl IRect {
    pub const fn new(x0: i64, y0: i64, x1: i64, y1: i64) -> IRect {
        IRect { x0, y0, x1, y1 }
    }

    /// The rectangle from the origin to `(w, h)`.
    pub const fn from_size(w: i64, h: i64) -> IRect {
        IRect::new(0, 0, w, h)
    }

    pub fn width(&self) -> i64 {
        (self.x1 - self.x0).max(0)
    }

    pub fn height(&self) -> i64 {
        (self.y1 - self.y0).max(0)
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    /// The smallest rectangle containing both (an empty rectangle contributes nothing).
    pub fn union(&self, other: &IRect) -> IRect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        IRect::new(
            self.x0.min(other.x0),
            self.y0.min(other.y0),
            self.x1.max(other.x1),
            self.y1.max(other.y1),
        )
    }

    pub fn intersect(&self, other: &IRect) -> IRect {
        IRect::new(
            self.x0.max(other.x0),
            self.y0.max(other.y0),
            self.x1.min(other.x1),
            self.y1.min(other.y1),
        )
    }

    pub fn contains(&self, x: i64, y: i64) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}
