//! The versions and units the server writes and the browser reads, kept in
//! one place so a change to one side cannot be missed on the other.

/// In every visual keyframe; deltas follow one, so they need none.
pub const WIRE_VERSION: u8 = 16;
/// The detail stream's keyframes.
pub const DETAIL_VERSION: u8 = 7;
/// The terrain resource's layout.
pub const TERRAIN_VERSION: u8 = 4;
/// Engine heights are u16 in these units.
pub const HEIGHT_UNITS_PER_METRE: f32 = 92.0;
