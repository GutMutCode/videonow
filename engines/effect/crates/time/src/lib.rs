//! Studio: EffectCraft's time is FilmCraft's time. The two crates were the same design (integer
//! ticks at 254 016 000 000 per second, rational frame rates, SMPTE timecode); VideoNow keeps one
//! implementation, `filmcraft-time`, so a time from the timeline and a time from a composition are
//! the same type and convert without loss. This crate stays so EffectCraft's code keeps its paths.

pub use filmcraft_time::*;
