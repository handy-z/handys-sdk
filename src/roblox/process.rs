use std::cell::Cell;
use std::sync::Arc;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::{
    EnumProcessModulesEx, GetModuleBaseNameW, GetModuleFileNameExW, LIST_MODULES_ALL,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_VM_READ, WaitForSingleObject,
};

use super::memory::{ClassNameCache, Memory};
use super::version::file_version_of;
use crate::util::text::from_wide;

const EXE: &str = "RobloxPlayerBeta.exe";

struct Handle(HANDLE);
// SAFETY: a process handle may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

thread_local! {
    static READS: Cell<u64> = const { Cell::new(0) };
}

/// Read-only handle to the running Roblox client (clones share the handle and the class-name cache).
/// The default value is attached to nothing: every read fails.
#[derive(Clone, Default)]
pub struct RobloxProcess {
    h: Option<Arc<Handle>>,
    classes: Arc<ClassNameCache>,
    pid: u32,
    base: usize,
    exe_path: String,
    file_version: String,
}

impl RobloxProcess {
    pub fn find() -> Option<Self> {
        let pid = find_pid()?;
        let h = unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .ok()?;
        let handle = Arc::new(Handle(h));

        let mut mods = vec![HMODULE::default(); 1024];
        let mut need = 0u32;
        unsafe {
            EnumProcessModulesEx(
                h,
                mods.as_mut_ptr(),
                size_of_val(&mods[..]) as u32,
                &mut need,
                LIST_MODULES_ALL,
            )
        }
        .ok()?;
        mods.truncate(need as usize / size_of::<HMODULE>());
        let mut name = [0u16; 260];
        let exe = mods
            .iter()
            .copied()
            .find(|m| {
                let n = unsafe { GetModuleBaseNameW(h, Some(*m), &mut name) } as usize;
                n > 0 && from_wide(&name[..n]).eq_ignore_ascii_case(EXE)
            })
            .or_else(|| mods.first().copied())?; // fallback: first module is the main image

        let mut path = [0u16; 520];
        let len = unsafe { GetModuleFileNameExW(Some(h), Some(exe), &mut path) } as usize;
        if len == 0 {
            return None;
        }
        let exe_path = from_wide(&path[..len]);
        let file_version = file_version_of(&exe_path)?;
        Some(Self {
            h: Some(handle),
            classes: Arc::default(),
            pid,
            base: exe.0 as usize,
            exe_path,
            file_version,
        })
    }

    /// False once the process has exited (or for the default value).
    pub fn alive(&self) -> bool {
        self.h
            .as_ref()
            .is_some_and(|h| unsafe { WaitForSingleObject(h.0, 0) } == WAIT_TIMEOUT)
    }

    /// Waits up to `ms` for the process to exit; true when it has.
    pub(crate) fn wait_exit(&self, ms: u32) -> bool {
        match &self.h {
            Some(h) => (unsafe { WaitForSingleObject(h.0, ms.min(u32::MAX - 1)) }) == WAIT_OBJECT_0,
            None => {
                std::thread::sleep(std::time::Duration::from_millis(ms.into()));
                false
            }
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
    /// The Roblox module base.
    pub fn base(&self) -> usize {
        self.base
    }
    pub fn exe_path(&self) -> &str {
        &self.exe_path
    }
    pub fn file_version(&self) -> &str {
        &self.file_version
    }

    /// `read()` calls made by the calling thread, over every RobloxProcess; for measuring what a step costs.
    pub fn reads_on_this_thread() -> u64 {
        READS.get()
    }
}

impl Memory for RobloxProcess {
    fn read(&self, addr: usize, out: &mut [u8]) -> bool {
        READS.set(READS.get() + 1);
        let Some(h) = &self.h else { return false };
        let mut got = 0usize;
        unsafe {
            ReadProcessMemory(
                h.0,
                addr as *const _,
                out.as_mut_ptr().cast(),
                out.len(),
                Some(&mut got),
            )
        }
        .is_ok()
            && got == out.len()
    }

    fn class_names(&self) -> Option<&ClassNameCache> {
        Some(&self.classes)
    }
}

fn find_pid() -> Option<u32> {
    unsafe {
        let snap = Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?);
        let mut pe = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap.0, &mut pe).is_ok();
        while ok {
            if from_wide(&pe.szExeFile).eq_ignore_ascii_case(EXE) {
                return Some(pe.th32ProcessID);
            }
            ok = Process32NextW(snap.0, &mut pe).is_ok();
        }
        None
    }
}
