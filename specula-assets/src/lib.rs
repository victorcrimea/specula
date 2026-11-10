//! What specula-tools' bake-assets writes and the web client reads: the
//! split asset files, the engine maths both sides place models with, and
//! the BC decoder for devices without BC textures.

pub mod bc;
pub mod bytes;
pub mod command;
pub mod files;
pub mod half;
pub mod math;
pub mod shore;
pub mod variation;
pub mod wire;
