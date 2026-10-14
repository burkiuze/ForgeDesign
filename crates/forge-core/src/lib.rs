//! # forge-core
//!
//! The engine behind Forge Design. Everything in here is GUI-agnostic and
//! can be driven headlessly (see `forge-cli`).
//!
//! Key design decisions:
//!
//! * **Tiled, sparse, copy-on-write storage** ([`buffer::TiledBuffer`]).
//!   Images are split into 64×64 tiles held behind `Arc`. Fully transparent
//!   tiles are never allocated, and cloning a layer only clones pointers.
//!   This makes undo snapshots almost free: a history step only pays for
//!   the tiles that actually changed.
//! * **Parallel everything**: compositing, filters and adjustments run tile-
//!   or row-parallel through `rayon`.
//! * **Photoshop-compatible blending**: all 27 Photoshop blend modes,
//!   implemented after the W3C Compositing & Blending spec.

pub mod adjust;
pub mod blend;
pub mod brush;
pub mod buffer;
pub mod color;
pub mod composite;
pub mod document;
pub mod fill;
pub mod filters;
pub mod geom;
pub mod history;
pub mod layer;
pub mod selection;
pub mod transform;

pub use blend::BlendMode;
pub use buffer::{TiledBuffer, TILE_SIZE};
pub use color::{Color, Rgba8};
pub use document::{DocState, Document};
pub use geom::{IRect, Point};
pub use layer::{Layer, LayerId};
