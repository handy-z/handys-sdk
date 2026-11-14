//! What this process costs the machine: CPU, RAM, and GPU use (the GPU counters Task Manager reads).

use std::time::Instant;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Performance::{
    PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessTimes};
use windows::core::w;

/// One sample: CPU as a percent of the whole machine (all cores, as Task Manager shows it), RAM (working set) in
/// bytes, GPU as its busiest engine's percent; None for what could not be read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub cpu: Option<f32>,
    pub ram: Option<u64>,
    pub gpu: Option<f32>,
}

/// Samples this process's Usage; CPU and GPU are averages since the last sample, so the first gives none of them.
/// GPU counters enumerate every engine on the machine: a sample costs a few milliseconds, so take one a second or
/// less.
pub struct UsageMeter {
    last_cpu: Option<(Instant, u64)>, // when, the process's CPU time in 100 ns units
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    instance: String, // "pid_<this process>_": the GPU counters' instances that are this process's
}

impl Default for UsageMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageMeter {
    pub fn new() -> Self {
        let pid = unsafe { GetCurrentProcessId() };
        let mut query = PDH_HQUERY::default();
        let mut engine = PDH_HCOUNTER::default();
        unsafe {
            if PdhOpenQueryW(None, 0, &mut query) == 0 {
                // every process's instances: a counter for this one's alone fails until it has used the GPU
                let _ = PdhAddEnglishCounterW(query, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut engine);
                let _ = PdhCollectQueryData(query); // the baseline the first utilization is measured from
            }
        }
        Self {
            last_cpu: None,
            query,
            engine,
            instance: format!("pid_{pid}_"),
        }
    }

    pub fn sample(&mut self) -> Usage {
        let me = unsafe { GetCurrentProcess() };
        let mut out = Usage::default();
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        if unsafe { GetProcessTimes(me, &mut created, &mut exited, &mut kernel, &mut user) }.is_ok() {
            let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
            let now = (Instant::now(), ticks(kernel) + ticks(user));
            if let Some((t0, c0)) = self.last_cpu {
                let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
                out.cpu = cpu_percent(now.1 - c0, (now.0 - t0).as_secs_f32(), cores);
            }
            self.last_cpu = Some(now);
        }
        let mut mem = PROCESS_MEMORY_COUNTERS::default();
        let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if unsafe { GetProcessMemoryInfo(me, &mut mem, size) }.is_ok() {
            out.ram = Some(mem.WorkingSetSize as u64);
        }
        if !self.query.is_invalid() && unsafe { PdhCollectQueryData(self.query) } == 0 {
            out.gpu = counter_items(self.engine, &self.instance).map(|items| busiest_engine(&items) as f32);
        }
        out
    }
}

impl Drop for UsageMeter {
    fn drop(&mut self) {
        if !self.query.is_invalid() {
            unsafe { PdhCloseQuery(self.query) };
        }
    }
}

/// CPU time used (100 ns units) over `secs` of wall time, as a percent of all `cores`; None without time passed.
fn cpu_percent(used: u64, secs: f32, cores: f32) -> Option<f32> {
    (secs > 0.0).then(|| (used as f32 / 1e7 / secs / cores * 100.0).clamp(0.0, 100.0))
}

/// Task Manager's GPU figure: utilization summed per engine type ("..._engtype_3D"), the busiest type's.
fn busiest_engine(items: &[(String, f64)]) -> f64 {
    let mut by_type: Vec<(&str, f64)> = Vec::new();
    for (name, v) in items {
        let kind = name.rsplit_once("engtype_").map_or("", |(_, t)| t);
        match by_type.iter_mut().find(|(k, _)| *k == kind) {
            Some(e) => e.1 += v,
            None => by_type.push((kind, *v)),
        }
    }
    by_type.iter().map(|e| e.1).fold(0.0, f64::max).clamp(0.0, 100.0)
}

/// A wildcard counter's instances starting with `prefix` that have valid values, as (instance name, value); None when
/// it can't be read. No instance of this process means it hasn't used the GPU: an empty list, so 0.
fn counter_items(counter: PDH_HCOUNTER, prefix: &str) -> Option<Vec<(String, f64)>> {
    if counter.is_invalid() {
        return None;
    }
    let (mut bytes, mut count) = (0u32, 0u32);
    let status = unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None) };
    if status != PDH_MORE_DATA {
        return None;
    }
    // the items, then the names they point into: one buffer of `bytes`, aligned for the items
    let item = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    let mut buf = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); (bytes as usize).div_ceil(item)];
    let status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, Some(buf.as_mut_ptr()))
    };
    if status != 0 {
        return None;
    }
    Some(
        buf.iter()
            .take(count as usize)
            .filter(|i| i.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA)
            .map(|i| {
                let name = unsafe { i.szName.to_string() }.unwrap_or_default();
                (name, unsafe { i.FmtValue.Anonymous.doubleValue })
            })
            .filter(|(name, _)| name.starts_with(prefix))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_cpu_is_a_share_of_all_cores() {
        // one core busy for the whole second on a 4-core machine: 25%
        assert_eq!(cpu_percent(10_000_000, 1.0, 4.0), Some(25.0));
        assert_eq!(cpu_percent(0, 1.0, 4.0), Some(0.0));
        assert_eq!(cpu_percent(5, 0.0, 4.0), None);
    }

    #[test]
    fn usage_gpu_is_the_busiest_engine_type() {
        let items = [
            ("pid_1_luid_0x1_phys_0_eng_0_engtype_3D".to_owned(), 3.0),
            ("pid_1_luid_0x1_phys_0_eng_1_engtype_3D".to_owned(), 2.0),
            ("pid_1_luid_0x1_phys_0_eng_5_engtype_Copy".to_owned(), 4.0),
        ];
        assert_eq!(busiest_engine(&items), 5.0); // 3D: 3 + 2
        assert_eq!(busiest_engine(&[]), 0.0);
    }

    #[test]
    fn usage_samples_this_process() {
        let mut meter = UsageMeter::new();
        let first = meter.sample();
        assert!(first.cpu.is_none() && first.ram.is_some_and(|r| r > 1 << 20));
        std::thread::sleep(std::time::Duration::from_millis(50));
        let t = Instant::now();
        let second = meter.sample();
        assert!(second.cpu.is_some_and(|c| (0.0..=100.0).contains(&c)));
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed()); // cheap enough for once a second
    }
}
