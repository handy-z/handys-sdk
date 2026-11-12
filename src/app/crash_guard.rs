use std::sync::atomic::{AtomicPtr, Ordering};

use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS, SetUnhandledExceptionFilter,
};

// The filter is a plain function, so the callback lives here.
static RELEASE: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

fn call_release() {
    let p = RELEASE.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: only ever set from a `fn()` in on_crash.
        unsafe { std::mem::transmute::<*mut (), fn()>(p)() };
    }
}

unsafe extern "system" fn filter(_: *const EXCEPTION_POINTERS) -> i32 {
    call_release();
    EXCEPTION_CONTINUE_SEARCH
}

/// One line in crash.log: GUI apps have no console, so without it a panic vanishes. Every error is ignored.
fn log_panic(info: &std::panic::PanicHookInfo) {
    use std::io::Write;
    if !crate::util::paths::has_app_name() {
        return; // app_data_dir would panic
    }
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let at = info
        .location()
        .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
    let msg = info.payload_as_str().unwrap_or("(non-string panic)");
    let line = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} panic: {}{}\n",
        t.wYear,
        t.wMonth,
        t.wDay,
        t.wHour,
        t.wMinute,
        t.wSecond,
        msg.replace('\n', " "),
        at
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(crate::util::paths::app_data_dir().join("crash.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Installs the process-wide unhandled-exception filter and a panic hook; both call `release` first (e.g. to lift
/// held keys), then the default crash handling continues (the previous panic hook, then the abort). Call once, early
/// in main.
pub fn on_crash(release: fn()) {
    RELEASE.store(release as *mut (), Ordering::Release);
    unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        call_release();
        log_panic(info);
        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    // Compile-only: never installed in tests (it would replace the test harness's panic hook).
    const _: fn(fn()) = super::on_crash;
}
