//! The shared Roblox layer under every handy's tool: finds the client, reads its memory, walks the instance tree,
//! works out which build is running and fetches the matching offsets. Windows x64 only.
//! Nothing attaches to Roblox until the first call that reads it.
#![cfg(windows)]
// Callbacks are `Box<dyn Fn(..) + Send + Sync>` by convention (CLAUDE.md), which this lint flags everywhere.
#![allow(clippy::type_complexity)]

pub mod app;
pub mod input;
pub mod roblox;
pub mod testing;
pub mod ui;
pub mod util;
