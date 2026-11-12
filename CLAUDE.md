# CLAUDE.md

handy-sdk (repo handys-sdk): the shared Rust library behind handy''s Roblox tools. Windows-only; reads Roblox process memory and wraps the Win32 pieces the apps share (input, windows, UI, threads). The apps (`99n`, `fisch`, `vbl`, `bms`, `esp`) depend on it from GitHub.

## Build and test

```
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
cargo fmt --check
```

Tests named `live_*` need a running Roblox and the network: mark them `#[ignore]` and never run them unasked.

Standalone crate (empty `[workspace]`, own `Cargo.lock`). In the shared development folder it is also reached by the apps through the root `.cargo/config.toml` patch (`../sdk`), so a change here shows in the apps without a push.

## Safety (never skip)

- Never send input to Roblox in a public match.
- Never change input timing the apps rely on (hold lengths, gaps, sending only while Roblox is foreground) and never leave keys held after a crash, close or toggle-off.
- Low-level hook callbacks stay O(1) and non-blocking (atomics only).
- **Nothing game-specific in the SDK.**

## Conventions

Each module is `src/<folder>/<file>.rs`, with its tests in a `#[cfg(test)] mod tests` at the bottom of the same file. Comments explain why, not what.

- **Names:** snake_case functions, `UPPER_CASE` consts.
- **Addresses** are `usize`; 0 means none. Read helpers return 0 or `None` on failure, so chains need no checks in between.
- **Memory:** functions take `m: &dyn Memory, o: &Offsets`. Typed reads are `m.get::<T>(addr)` (T: `Pod`) and `m.read_slice`. Structs read from Roblox are `#[repr(C)]` + `unsafe impl Pod`. Speed is the number of `Memory::read` calls (each is one ReadProcessMemory round trip); the `read_cost` tests pin them, so never let one rise unnoticed.
- **Offsets:** `o["Class.Field"]` and `o.at(base, "Class.Field")` panic on a missing key (a bug in an app's required list). Every key an app reads must be in its `required_keys()`. Code that must survive missing keys checks `o.has_all(&KEYS)` first; never `catch_unwind`.
- **Errors:** fallible functions return `Result<T, String>`. Bugs are panics; `panic = "abort"` is set, so a panic ends the process after the crash guard has released held keys and written `crash.log`.
- **Strings:** `&str` / `String` everywhere, UTF-16 only at the Win32 call (`util::text::{wide, wide_path, from_wide}`). Paths are `&Path` / `PathBuf`.
- **Win32:** the `windows` crate only (add features to `Cargo.toml` as needed). Wrap handles so they close on drop. Keep `unsafe` blocks small.
- **JSON:** `serde_json`.
- **Threads:** a stop flag is `stop: &AtomicBool`; long-lived threads are `util::thread::StopThread`. State shared across threads lives behind `Arc`; callbacks are `Box<dyn Fn(..) + Send + Sync>`.
- **Time:** `std::time::{Duration, Instant}`.
- **Tests:** `crate::testing::FakeMemory` is the byte-map `Memory`; inject `HttpGet` so tests never hit the network; each test uses its own temp dir (tests run in parallel).
- No new dependencies without a reason, no abstractions without two callers, no dead code.
