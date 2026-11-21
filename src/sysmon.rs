//! The optional system monitor (Settings → System monitor, off by default):
//! CPU, memory, GPU, network and disk use, sampled once a second on a
//! background thread and shown in the widget.
//!
//! - CPU: `GetSystemTimes`, busy time against all time since the last sample.
//! - Memory: `GlobalMemoryStatusEx`, used against total physical memory.
//! - Network: `GetIfTable2`, the byte counters of hardware adapters (not
//!   their filter copies or virtual adapters, which would count traffic twice).
//! - Disk: the performance counter `\PhysicalDisk(_Total)\% Idle Time`; busy
//!   is the rest, like "active time" in Task Manager. Read and written bytes
//!   per second from `Disk Read Bytes/sec` and `Disk Write Bytes/sec`.
//! - GPU: the performance counters `\GPU Engine(*)\Utilization Percentage`,
//!   one per process and engine; summed per engine, the busiest engine, like
//!   Task Manager. Video memory: `\GPU Adapter Memory(*)\Dedicated Usage` of
//!   the graphics card with the most memory, its size from DXGI.
//!
//! Nothing is sampled while every value is off. The demo shows made-up values.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// The values, as bits of the switch shared with the sampler.
pub const CPU: u8 = 1;
pub const RAM: u8 = 2;
pub const GPU: u8 = 4;
pub const NET: u8 = 8;
pub const DISK: u8 = 16;

/// One second's numbers; `None` where a value is off or couldn't be read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// Percent, 0 to 100.
    pub cpu: Option<f32>,
    pub ram: Option<f32>,
    pub gpu: Option<f32>,
    pub disk: Option<f32>,
    /// Physical memory in use and in all, in bytes.
    pub ram_bytes: Option<(f64, f64)>,
    /// Video memory in use and, when known, the card's size, in bytes.
    pub vram: Option<Amount>,
    /// Bytes per second.
    pub up: Option<f64>,
    pub down: Option<f64>,
    pub disk_read: Option<f64>,
    pub disk_write: Option<f64>,
}

/// Memory in use and, when known, in all, in bytes.
pub type Amount = (f64, Option<f64>);

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// An amount of memory, in GB as Task Manager counts them (1024³ bytes):
/// `7.8 GB`, `21.4 GB`, `128 GB`.
pub fn gb(bytes: f64) -> String {
    let g = bytes.max(0.0) / GIB;
    if g >= 99.95 { format!("{g:.0} GB") } else { format!("{g:.1} GB") }
}

/// The LUID in a GPU counter's instance name (`luid_0x00000000_0x0000D2E1_…`),
/// as (high, low).
pub fn luid_of(name: &str) -> Option<(u32, u32)> {
    let (_, rest) = name.split_once("luid_0x")?;
    let (high, rest) = rest.split_once("_0x")?;
    let low = rest.split('_').next()?;
    Some((u32::from_str_radix(high, 16).ok()?, u32::from_str_radix(low, 16).ok()?))
}

/// Video memory in use on the card with the most memory (`cards`: LUID and
/// size), from the per-adapter counters `items`. Without cards: the adapter
/// using the most, its size unknown.
pub fn main_vram(items: &[(String, f64)], cards: &[((u32, u32), u64)]) -> Option<Amount> {
    let used_by = |luid| items.iter().filter(|(n, _)| luid_of(n) == Some(luid)).map(|(_, v)| *v).sum::<f64>();
    if let Some((luid, size)) = cards.iter().max_by_key(|(_, size)| *size) {
        if items.iter().any(|(n, _)| luid_of(n) == Some(*luid)) {
            return Some((used_by(*luid), Some(*size as f64)));
        }
    }
    let mut adapters: Vec<(u32, u32)> = items.iter().filter_map(|(n, _)| luid_of(n)).collect();
    adapters.dedup();
    adapters
        .into_iter()
        .map(used_by)
        .fold(None, |best: Option<f64>, v| Some(best.map_or(v, |b| b.max(v))))
        .map(|v| (v, None))
}

/// A rate in a short form: `0 KB/s`, `820 KB/s`, `1.2 MB/s`, `18 MB/s`, `1.1 GB/s`.
pub fn rate(bytes_per_sec: f64) -> String {
    let b = bytes_per_sec.max(0.0);
    if b >= 1e9 {
        format!("{:.1} GB/s", b / 1e9)
    } else if b >= 10e6 {
        format!("{:.0} MB/s", b / 1e6)
    } else if b >= 1e6 {
        format!("{:.1} MB/s", b / 1e6)
    } else {
        format!("{:.0} KB/s", b / 1e3)
    }
}

/// The busiest GPU engine, from per-process engine counters named like
/// `pid_1234_luid_0x0_0x1_phys_0_eng_3_engtype_3D`: summed per engine (the
/// name without its process), then the largest, at most 100.
pub fn busiest_engine<'a>(items: impl Iterator<Item = (&'a str, f64)>) -> f64 {
    let mut per_engine: Vec<(&str, f64)> = Vec::new();
    for (name, value) in items {
        let engine = name.find("luid_").and_then(|i| name.get(i..)).unwrap_or(name);
        match per_engine.iter_mut().find(|(e, _)| *e == engine) {
            Some((_, v)) => *v += value,
            None => per_engine.push((engine, value)),
        }
    }
    per_engine.iter().map(|(_, v)| *v).fold(0.0, f64::max).min(100.0)
}

/// Busy share from two readings of (idle, total) time counters, in percent.
pub fn busy_percent(before: (u64, u64), after: (u64, u64)) -> Option<f32> {
    let total = after.1.checked_sub(before.1).filter(|t| *t > 0)?;
    let idle = after.0.saturating_sub(before.0).min(total);
    Some(((total - idle) as f64 / total as f64 * 100.0) as f32)
}

/// Starts the sampler. Values switched on in the returned bits are sampled
/// once a second and handed to `report`.
pub fn spawn(report: impl Fn(Sample) + Send + 'static) -> Arc<AtomicU8> {
    let on = Arc::new(AtomicU8::new(0));
    let shared = on.clone();
    std::thread::spawn(move || {
        crate::win::background_thread();
        let mut s = Sampler::default();
        loop {
            let bits = shared.load(Ordering::Relaxed);
            if bits == 0 {
                // Release what the counters hold; start afresh when switched on again.
                s = Sampler::default();
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            let started = Instant::now();
            let sample = if crate::demo::active() { s.made_up(bits) } else { s.sample(bits) };
            report(sample);
            std::thread::sleep(Duration::from_secs(1).saturating_sub(started.elapsed()));
        }
    });
    on
}

#[derive(Default)]
struct Sampler {
    cpu: Option<(u64, u64)>,
    net: Option<(Instant, u64, u64)>,
    /// The counters' query, and whether it was opened for the disk and the GPU.
    pdh: Option<((bool, bool), pdh::Query)>,
    /// The graphics cards (LUID, dedicated memory), read once.
    cards: Option<Vec<((u32, u32), u64)>>,
    /// Seconds sampled, for the demo's made-up curves.
    tick: u32,
}

impl Sampler {
    fn sample(&mut self, bits: u8) -> Sample {
        let mut out = Sample::default();
        if bits & CPU != 0 {
            let now = cpu_times();
            out.cpu = self.cpu.zip(now).and_then(|(a, b)| busy_percent(a, b));
            self.cpu = now;
        }
        if bits & RAM != 0 {
            if let Some((used, total)) = memory() {
                out.ram = Some((used / total * 100.0) as f32);
                out.ram_bytes = Some((used, total));
            }
        }
        if bits & NET != 0 {
            if let Some((down, up)) = net_bytes() {
                let now = Instant::now();
                if let Some((then, d0, u0)) = self.net {
                    let secs = now.duration_since(then).as_secs_f64().max(0.1);
                    // Counters restart when an adapter does: skip that second.
                    if down >= d0 && up >= u0 {
                        out.down = Some((down - d0) as f64 / secs);
                        out.up = Some((up - u0) as f64 / secs);
                    }
                }
                self.net = Some((now, down, up));
            }
        }
        if bits & (GPU | DISK) != 0 {
            // Only the counters of what's switched on: the GPU's have an
            // instance per process and engine, costly to collect every second.
            let wanted = (bits & DISK != 0, bits & GPU != 0);
            if self.pdh.as_ref().is_none_or(|(opened, _)| *opened != wanted) {
                let q = pdh::Query::open(wanted.0, wanted.1);
                // Rates need two readings: a first one now, so this sample isn't empty.
                q.collect();
                std::thread::sleep(std::time::Duration::from_millis(250));
                self.pdh = Some((wanted, q));
            }
            let Some((_, q)) = &self.pdh else { return out };
            q.collect();
            if bits & DISK != 0 {
                out.disk = q.disk_busy();
                (out.disk_read, out.disk_write) = q.disk_rates();
            }
            if bits & GPU != 0 {
                out.gpu = q.gpu_busy();
                let cards = self.cards.get_or_insert_with(video_cards);
                out.vram = q.vram().and_then(|items| main_vram(&items, cards));
            }
        } else {
            self.pdh = None;
        }
        out
    }

    /// Smooth made-up values for the demo.
    fn made_up(&mut self, bits: u8) -> Sample {
        self.tick += 1;
        let t = self.tick as f32;
        let wave = |base: f32, amp: f32, speed: f32| (base + amp * (t * speed).sin()).clamp(0.0, 100.0);
        let on = |bit: u8| bits & bit != 0;
        let ram = on(RAM).then(|| wave(58.0, 2.0, 0.1));
        Sample {
            cpu: on(CPU).then(|| wave(24.0, 14.0, 0.7)),
            ram,
            ram_bytes: ram.map(|r| (f64::from(r) / 100.0 * 32.0 * GIB, 32.0 * GIB)),
            gpu: on(GPU).then(|| wave(31.0, 22.0, 0.4)),
            vram: on(GPU).then(|| (f64::from(wave(30.0, 6.0, 0.2)) / 100.0 * 16.0 * GIB, Some(16.0 * GIB))),
            disk: on(DISK).then(|| wave(6.0, 5.0, 1.3)),
            disk_read: on(DISK).then(|| f64::from(wave(30.0, 28.0, 0.8)) * 60_000.0),
            disk_write: on(DISK).then(|| f64::from(wave(20.0, 18.0, 1.1)) * 25_000.0),
            up: on(NET).then(|| f64::from(wave(40.0, 35.0, 0.9)) * 30_000.0),
            down: on(NET).then(|| f64::from(wave(50.0, 45.0, 0.5)) * 400_000.0),
        }
    }
}

fn filetime(t: windows::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// (idle, total) CPU time of all processors, in 100 ns.
fn cpu_times() -> Option<(u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: valid out-pointers.
    unsafe { windows::Win32::System::Threading::GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }
        .ok()?;
    // Kernel time includes idle time.
    Some((filetime(idle), filetime(kernel) + filetime(user)))
}

/// Physical memory in use and in all, in bytes.
fn memory() -> Option<(f64, f64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: the struct with its size set, as the call requires.
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    (m.ullTotalPhys > 0).then(|| ((m.ullTotalPhys - m.ullAvailPhys) as f64, m.ullTotalPhys as f64))
}

/// The hardware graphics cards: LUID (high, low) and dedicated memory. DXGI
/// only lists them; no device is created, so no graphics driver is loaded.
fn video_cards() -> Vec<((u32, u32), u64)> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
    let mut cards = Vec::new();
    // SAFETY: COM calls on interfaces DXGI returned; they are released when dropped.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return cards };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = adapter.GetDesc1() else { continue };
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 {
                cards.push(((d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart), d.DedicatedVideoMemory as u64));
            }
        }
    }
    cards
}

/// Bytes received and sent so far by the hardware adapters that are up.
fn net_bytes() -> Option<(u64, u64)> {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    const HARDWARE: u8 = 1;
    const FILTER: u8 = 2;
    let mut out = std::mem::MaybeUninit::<*mut MIB_IF_TABLE2>::uninit();
    // SAFETY: the call allocates the table and sets the pointer when it succeeds;
    // the rows follow the header (the type declares one, there are NumEntries),
    // are read through a raw pointer within that count, and the table is freed
    // with FreeMibTable.
    unsafe {
        if GetIfTable2(out.as_mut_ptr()).is_err() {
            return None;
        }
        let table = out.assume_init();
        if table.is_null() {
            return None;
        }
        let first =
            std::ptr::addr_of!((*table).Table).cast::<windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2>();
        let rows = std::slice::from_raw_parts(first, (*table).NumEntries as usize);
        let (mut down, mut up) = (0u64, 0u64);
        for r in rows {
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            if flags & HARDWARE != 0 && flags & FILTER == 0 && r.OperStatus == IfOperStatusUp {
                down += r.InOctets;
                up += r.OutOctets;
            }
        }
        FreeMibTable(table as *const _);
        Some((down, up))
    }
}

/// The disk and GPU performance counters.
mod pdh {
    use windows::Win32::System::Performance::{
        PDH_FMT, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
        PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhGetFormattedCounterValue, PdhOpenQueryW,
    };
    use windows::core::w;

    /// Values above 100 % stay as they are (summed engines, idle time).
    const FORMAT: PDH_FMT = PDH_FMT(PDH_FMT_DOUBLE.0 | 0x8000);

    pub struct Query {
        query: Option<PDH_HQUERY>,
        disk_idle: Option<PDH_HCOUNTER>,
        disk_read: Option<PDH_HCOUNTER>,
        disk_write: Option<PDH_HCOUNTER>,
        gpu: Option<PDH_HCOUNTER>,
        vram: Option<PDH_HCOUNTER>,
    }

    impl Query {
        /// A query with the counters of the disk and of the GPU, as asked for.
        pub fn open(disk: bool, gpu: bool) -> Query {
            let mut query = PDH_HQUERY::default();
            // SAFETY: valid out-pointers; the query is closed in Drop. A counter
            // that doesn't exist (no GPU counters) stays None.
            unsafe {
                if PdhOpenQueryW(None, 0, &mut query) != 0 {
                    return Query {
                        query: None,
                        disk_idle: None,
                        disk_read: None,
                        disk_write: None,
                        gpu: None,
                        vram: None,
                    };
                }
                let add = |wanted: bool, path| {
                    let mut c = PDH_HCOUNTER::default();
                    (wanted && PdhAddEnglishCounterW(query, path, 0, &mut c) == 0).then_some(c)
                };
                let disk_idle = add(disk, w!("\\PhysicalDisk(_Total)\\% Idle Time"));
                let disk_read = add(disk, w!("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec"));
                let disk_write = add(disk, w!("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec"));
                let gpu_busy = add(gpu, w!("\\GPU Engine(*)\\Utilization Percentage"));
                let vram = add(gpu, w!("\\GPU Adapter Memory(*)\\Dedicated Usage"));
                Query { query: Some(query), disk_idle, disk_read, disk_write, gpu: gpu_busy, vram }
            }
        }

        pub fn collect(&self) {
            if let Some(q) = self.query {
                // SAFETY: an open query.
                unsafe { PdhCollectQueryData(q) };
            }
        }

        /// The value of a counter with one instance.
        fn value(c: Option<PDH_HCOUNTER>) -> Option<f64> {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            // SAFETY: a counter of the open query; the value is read as the double it was asked for.
            unsafe {
                if PdhGetFormattedCounterValue(c?, FORMAT, None, &mut v) != 0 || v.CStatus > 1 {
                    return None;
                }
                Some(v.Anonymous.doubleValue)
            }
        }

        pub fn disk_busy(&self) -> Option<f32> {
            Self::value(self.disk_idle).map(|idle| (100.0 - idle).clamp(0.0, 100.0) as f32)
        }

        /// Bytes read and written per second.
        pub fn disk_rates(&self) -> (Option<f64>, Option<f64>) {
            (Self::value(self.disk_read), Self::value(self.disk_write))
        }

        pub fn gpu_busy(&self) -> Option<f32> {
            let items = Self::items(self.gpu?)?;
            Some(super::busiest_engine(items.iter().map(|(n, v)| (n.as_str(), *v))) as f32)
        }

        /// Dedicated video memory in use, per adapter instance.
        pub fn vram(&self) -> Option<Vec<(String, f64)>> {
            Self::items(self.vram?)
        }

        /// The values of a counter with many instances, by instance name.
        fn items(c: PDH_HCOUNTER) -> Option<Vec<(String, f64)>> {
            let (mut size, mut count) = (0u32, 0u32);
            // SAFETY: the first call asks for the size; the buffer is allocated
            // with it (8-byte aligned) and read within the item count.
            unsafe {
                if PdhGetFormattedCounterArrayW(c, FORMAT, &mut size, &mut count, None) != PDH_MORE_DATA {
                    return None;
                }
                let mut buf = vec![0u64; (size as usize).div_ceil(8)];
                let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
                if PdhGetFormattedCounterArrayW(c, FORMAT, &mut size, &mut count, Some(items)) != 0 {
                    return None;
                }
                let items = std::slice::from_raw_parts(items, count as usize);
                Some(
                    items
                        .iter()
                        .filter(|i| i.FmtValue.CStatus <= 1)
                        .map(|i| (i.szName.to_string().unwrap_or_default(), i.FmtValue.Anonymous.doubleValue))
                        .collect(),
                )
            }
        }
    }

    impl Drop for Query {
        fn drop(&mut self) {
            if let Some(q) = self.query {
                // SAFETY: the query opened in `open`, closed once.
                unsafe { PdhCloseQuery(q) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows' table of network adapters reads, on any PC (CI's runner too).
    #[test]
    fn reads_the_network_table() {
        let (down, up) = net_bytes().unwrap_or_default();
        assert!(net_bytes().is_some(), "GetIfTable2 failed");
        // Counters only grow.
        let (down2, up2) = net_bytes().unwrap_or_default();
        assert!(down2 >= down && up2 >= up);
    }

    /// Samples this PC: `cargo test samples_this_pc -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn samples_this_pc() {
        let mut s = Sampler::default();
        let all = CPU | RAM | GPU | NET | DISK;
        s.sample(all);
        std::thread::sleep(Duration::from_secs(1));
        let started = Instant::now();
        let x = s.sample(all);
        println!("{x:?} in {:?}", started.elapsed());
        assert!(x.cpu.is_some_and(|v| (0.0..=100.0).contains(&v)));
        assert!(x.ram.is_some_and(|v| v > 0.0 && v <= 100.0));
        assert!(x.up.is_some() && x.down.is_some());
    }

    #[test]
    fn formats_memory() {
        assert_eq!(gb(7.8 * GIB), "7.8 GB");
        assert_eq!(gb(21.44 * GIB), "21.4 GB");
        assert_eq!(gb(128.0 * GIB), "128 GB");
        assert_eq!(gb(-1.0), "0.0 GB");
    }

    #[test]
    fn finds_the_main_cards_video_memory() {
        assert_eq!(luid_of("luid_0x00000000_0x0000D2E1_phys_0"), Some((0, 0xD2E1)));
        assert_eq!(luid_of("pid_10_luid_0x00000001_0x0000AB_phys_0_eng_3"), Some((1, 0xAB)));
        assert_eq!(luid_of("_Total"), None);
        let items = vec![
            ("luid_0x00000000_0x0000D2E1_phys_0".to_string(), 2.0 * GIB),
            ("luid_0x00000000_0x0000F0B0_phys_0".to_string(), 0.1 * GIB),
        ];
        // The card with the most memory, whatever it uses.
        let cards = [((0, 0xF0B0), 128 << 20), ((0, 0xD2E1), 12 << 30)];
        assert_eq!(main_vram(&items, &cards), Some((2.0 * GIB, Some(12.0 * GIB))));
        // Size unknown: the adapter using the most.
        assert_eq!(main_vram(&items, &[]), Some((2.0 * GIB, None)));
        assert_eq!(main_vram(&[], &cards), None);
    }

    #[test]
    fn formats_rates() {
        assert_eq!(rate(0.0), "0 KB/s");
        assert_eq!(rate(820_400.0), "820 KB/s");
        assert_eq!(rate(1_234_000.0), "1.2 MB/s");
        assert_eq!(rate(18_400_000.0), "18 MB/s");
        assert_eq!(rate(1_100_000_000.0), "1.1 GB/s");
        assert_eq!(rate(-5.0), "0 KB/s");
    }

    #[test]
    fn finds_the_busiest_gpu_engine() {
        let items = [
            ("pid_10_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 20.0),
            ("pid_11_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 25.0),
            ("pid_10_luid_0x0_0x1_phys_0_eng_4_engtype_VideoDecode", 30.0),
        ];
        assert_eq!(busiest_engine(items.into_iter()), 45.0);
        assert_eq!(busiest_engine([("pid_1_luid_x_eng_0", 80.0), ("pid_2_luid_x_eng_0", 70.0)].into_iter()), 100.0);
        assert_eq!(busiest_engine(std::iter::empty()), 0.0);
    }

    /// Samples this PC (read only), the way the sampler does every second: what's
    /// off stays empty, what's on is in range.
    #[test]
    fn samples_what_is_switched_on() {
        let mut s = Sampler::default();
        // Nothing on: nothing read.
        assert_eq!(s.sample(0), Sample::default());
        assert!(s.pdh.is_none());
        // The CPU needs two readings, the network too.
        let first = s.sample(CPU | RAM | NET);
        assert_eq!(first.cpu, None);
        assert_eq!((first.up, first.down), (None, None));
        assert!(first.ram.is_some_and(|v| v > 0.0 && v <= 100.0), "{first:?}");
        assert!(first.ram_bytes.is_some_and(|(used, total)| used > 0.0 && used <= total));
        std::thread::sleep(Duration::from_millis(150));
        let second = s.sample(CPU | NET);
        assert!(second.cpu.is_none_or(|v| (0.0..=100.0).contains(&v)), "{second:?}");
        assert!(second.up.is_some_and(|v| v >= 0.0) && second.down.is_some_and(|v| v >= 0.0));
        assert_eq!(second.ram, None);
        // The disk's and GPU's counters: opened for what's on, again when that changes.
        let disk = s.sample(DISK);
        assert!(disk.disk.is_none_or(|v| (0.0..=100.0).contains(&v)), "{disk:?}");
        assert_eq!(s.pdh.as_ref().map(|(opened, _)| *opened), Some((true, false)));
        let gpu = s.sample(GPU | DISK);
        assert!(gpu.gpu.is_none_or(|v| (0.0..=100.0).contains(&v)), "{gpu:?}");
        assert!(gpu.vram.is_none_or(|(used, size)| used >= 0.0 && size.is_none_or(|s| s > 0.0)));
        assert_eq!(s.pdh.as_ref().map(|(opened, _)| *opened), Some((true, true)));
        // Switched off: the counters are let go.
        s.sample(CPU);
        assert!(s.pdh.is_none());
    }

    #[test]
    fn reads_the_cpu_and_memory() {
        let (idle, total) = cpu_times().unwrap();
        assert!(idle <= total);
        let (used, total) = memory().unwrap();
        assert!(used > 0.0 && used < total);
        // The cards are only listed. CI's runner may have none, or a basic display
        // adapter without memory of its own, so nothing is said about their sizes.
        let cards = video_cards();
        assert!(cards.windows(2).all(|pair| pair.first().map(|c| c.0) != pair.get(1).map(|c| c.0)));
        let time = windows::Win32::Foundation::FILETIME { dwLowDateTime: 2, dwHighDateTime: 1 };
        assert_eq!(filetime(time), (1 << 32) | 2);
    }

    /// A query whose counters couldn't be added reads nothing.
    #[test]
    fn counters_not_asked_for_read_nothing() {
        let q = pdh::Query::open(false, false);
        q.collect();
        assert_eq!(q.disk_busy(), None);
        assert_eq!(q.disk_rates(), (None, None));
        assert_eq!(q.gpu_busy(), None);
        assert_eq!(q.vram(), None);
    }

    /// The demo's values: smooth, in range, only for what's on.
    #[test]
    fn makes_up_values_for_the_demo() {
        let mut s = Sampler::default();
        for _ in 0..50 {
            let x = s.made_up(CPU | RAM | GPU | NET | DISK);
            for v in [x.cpu, x.ram, x.gpu, x.disk] {
                assert!(v.is_some_and(|v| (0.0..=100.0).contains(&v)), "{x:?}");
            }
            assert!(x.ram_bytes.is_some_and(|(used, total)| used <= total));
            assert!(x.vram.is_some_and(|(used, size)| size.is_some_and(|s| used <= s)));
            assert!([x.up, x.down, x.disk_read, x.disk_write].iter().all(|v| v.is_some_and(|v| v >= 0.0)));
        }
        assert_eq!(s.tick, 50);
        let cpu = s.made_up(CPU);
        assert!(cpu.cpu.is_some() && cpu.ram.is_none() && cpu.vram.is_none() && cpu.up.is_none());
    }

    /// The sampler reads what's switched on, once a second, until it's switched off.
    #[test]
    fn the_sampler_reports_what_is_on() {
        let (tx, rx) = std::sync::mpsc::channel();
        let on = spawn(move |sample| {
            let _ = tx.send(sample);
        });
        assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err(), "sampled while off");
        on.store(RAM, Ordering::Relaxed);
        let sample = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(sample.ram.is_some() && sample.cpu.is_none());
        on.store(0, Ordering::Relaxed);
    }

    #[test]
    fn computes_busy_time() {
        assert_eq!(busy_percent((100, 1000), (150, 1100)), Some(50.0));
        assert_eq!(busy_percent((100, 1000), (200, 1100)), Some(0.0));
        assert_eq!(busy_percent((100, 1000), (100, 1000)), None);
        assert_eq!(busy_percent((100, 1000), (90, 900)), None);
    }
}
