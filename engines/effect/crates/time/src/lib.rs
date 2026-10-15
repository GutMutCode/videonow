//! Exact media time for EffectCraft.
//!
//! Studio: EffectCraft and FilmCraft kept two copies of the same time crate (same `Tick` unit,
//! 254 016 000 000 per second, same rational frame rates and timecode). In VideoNow both engines
//! use one: this crate re-exports `filmcraft-time`, which now also carries the EffectCraft-only
//! functions (`format_timecode_ae`, `format_feet_frames`, `parse_feet_frames`). Values cross
//! between the engines without conversion.

pub use filmcraft_time::*;
