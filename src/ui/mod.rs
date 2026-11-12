pub mod d2d;
pub mod dashboard;
pub mod gpu_overlay;
pub mod overlay;
pub mod theme;

/// Header dot colour: grey closed, amber on the way, green ready, red stuck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Busy,
    Good,
    Bad,
}
