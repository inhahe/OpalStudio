//! Opal Studio's document model.
//!
//! - [`tile`]: 256×256 tiles of premultiplied linear-light RGBA in half floats.
//! - [`raster`]: sparse, unbounded bitmaps made of tiles that are copied only when changed.
//! - [`document`]: the canvas and its tree of layers.
//! - [`mask`]: selections' masks, the shapes they are made of, and the blend through them.
//! - [`history`]: the undo tree, whose nodes are cheap snapshots of the document.
//! - [`composite`]: flattening layers into the projection, the cached image that is shown.
//! - [`resample`]: drawing a bitmap through a transform (image objects, resizing).
//! - [`selection`]: what is selected: the steps that made it, and the mask they come to.
//! - [`geom`]: rectangles and affine transforms.
//! - [`curve`]: editable curves, used by settings throughout the editor.
//! - [`units`]: units of length and the resolution that converts them to pixels.
//!
//! Everything a history state holds is shared through [`std::sync::Arc`], so taking a
//! snapshot copies pointers, and changing a tile copies that tile alone.

/// Gives a fieldless enum `ALL` (its values, in order), `NAMES` (their names, as the recipe
/// (serde), the API and the dialogs give them), and `named()` (the value of a name). The
/// tests of each check serde's names against `NAMES`.
macro_rules! named {
    ($t:ident: $($v:ident = $n:literal),+) => {
        impl $t {
            pub const ALL: [$t; [$($n),+].len()] = [$($t::$v),+];
            pub const NAMES: [&'static str; [$($n),+].len()] = [$($n),+];

            /// The one named `name` (see `NAMES`).
            pub fn named(name: &str) -> Option<$t> {
                $t::NAMES.iter().position(|n| *n == name).map(|i| $t::ALL[i])
            }
        }
    };
}

pub mod composite;
pub mod curve;
pub mod document;
pub mod expr;
pub mod geom;
pub mod history;
pub mod mask;
pub mod matting;
pub mod modify;
pub mod outline;
pub mod raster;
pub mod resample;
pub mod selection;
pub mod store;
pub mod symmetry;
pub mod tile;
pub mod units;

// The tests allocate tiles as the application does: through mimalloc (see opal-app's
// main.rs for why).
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
