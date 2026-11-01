//! Readings for Agora's own monitor keys: CPU, memory, disk, GPU and the
//! computer's IP address. Written for Agora; on Windows it uses the
//! system's own counters (the same ones Task Manager shows).

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One round of readings. `None` means "can't tell on this computer".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Percent busy, all cores.
    pub cpu: Option<f32>,
    /// Used and total bytes.
    pub memory: Option<(u64, u64)>,
    /// Per drive ("C:"): used and total bytes.
    pub disk_space: HashMap<String, (u64, u64)>,
    /// Per drive ("C:"): percent of time busy.
    pub disk_busy: HashMap<String, f32>,
    /// Percent busy, 3D engine.
    pub gpu: Option<f32>,
    /// Dedicated memory used, and the card's total when known.
    pub gpu_memory: Option<(u64, Option<u64>)>,
    pub public_ip: Option<String>,
    pub local_ip: Option<String>,
}

/// What the host tells the sampler.
pub struct Control {
    /// Monitor keys are on screen, so readings are wanted.
    pub wanted: Arc<AtomicBool>,
    /// Look the public address up again now.
    pub refresh_ip: Arc<AtomicBool>,
    /// Drives the disk keys ask about.
    pub drives: Arc<std::sync::Mutex<Vec<String>>>,
}

const EVERY: Duration = Duration::from_secs(2);
const IP_EVERY: Duration = Duration::from_secs(15 * 60);

/// Starts the sampling thread. Each round goes to `send`.
pub fn start(send: impl Fn(Snapshot) -> bool + Send + 'static) -> Control {
    let wanted = Arc::new(AtomicBool::new(false));
    let refresh_ip = Arc::new(AtomicBool::new(false));
    let drives = Arc::new(std::sync::Mutex::new(Vec::new()));
    let control = Control {
        wanted: wanted.clone(),
        refresh_ip: refresh_ip.clone(),
        drives: drives.clone(),
    };
    std::thread::Builder::new()
        .name("agora-stats".into())
        .spawn(move || {
            let mut sampler = platform::Sampler::new();
            let mut last_ip: Option<Instant> = None;
            let mut public_ip = None;
            loop {
                std::thread::sleep(EVERY);
                if !wanted.load(Ordering::Relaxed) {
                    continue;
                }
                let due = last_ip.is_none_or(|t| t.elapsed() > IP_EVERY);
                if due || refresh_ip.swap(false, Ordering::Relaxed) {
                    last_ip = Some(Instant::now());
                    public_ip = lookup_public_ip();
                }
                let want: Vec<String> = drives.lock().map(|d| d.clone()).unwrap_or_default();
                let mut s = sampler.read(&want);
                s.public_ip = public_ip.clone();
                s.local_ip = local_ip();
                if !send(s) {
                    break;
                }
            }
        })
        .ok();
    control
}

/// The address this computer uses on the local network. Nothing is sent:
/// connecting a UDP socket only picks the route.
fn local_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    match s.local_addr().ok()?.ip() {
        ip if ip.is_unspecified() => None,
        ip => Some(ip.to_string()),
    }
}

/// The address the internet sees. Asks OpenDNS's resolver for
/// "myip.opendns.com", which answers with the asker's address; if DNS is
/// blocked, falls back to a plain web lookup.
pub fn lookup_public_ip() -> Option<String> {
    dns_myip().or_else(http_myip)
}

fn dns_myip() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let id: u16 = (std::process::id() as u16) ^ 0x5a17;
    let mut q = Vec::with_capacity(40);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // recursion, 1 question
    for label in ["myip", "opendns", "com"] {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]); // end, type A, class IN
    let server = SocketAddr::from((Ipv4Addr::new(208, 67, 222, 222), 53));
    sock.send_to(&q, server).ok()?;
    let mut buf = [0u8; 512];
    let (n, from) = sock.recv_from(&mut buf).ok()?;
    if from != server {
        return None;
    }
    parse_a_answer(&buf[..n], id)
}

/// The first A record in a DNS reply to query `id`.
fn parse_a_answer(r: &[u8], id: u16) -> Option<String> {
    if r.len() < 12 || u16::from_be_bytes([r[0], r[1]]) != id || r[3] & 0x0f != 0 {
        return None;
    }
    let qd = u16::from_be_bytes([r[4], r[5]]) as usize;
    let an = u16::from_be_bytes([r[6], r[7]]) as usize;
    let mut i = 12;
    let skip_name = |mut i: usize| -> Option<usize> {
        loop {
            let len = *r.get(i)? as usize;
            if len == 0 {
                return Some(i + 1);
            }
            if len & 0xc0 == 0xc0 {
                return Some(i + 2);
            }
            i += 1 + len;
        }
    };
    for _ in 0..qd {
        i = skip_name(i)? + 4;
    }
    for _ in 0..an {
        i = skip_name(i)?;
        let typ = u16::from_be_bytes([*r.get(i)?, *r.get(i + 1)?]);
        let len = u16::from_be_bytes([*r.get(i + 8)?, *r.get(i + 9)?]) as usize;
        let data = r.get(i + 10..i + 10 + len)?;
        if typ == 1 && len == 4 {
            return Some(Ipv4Addr::new(data[0], data[1], data[2], data[3]).to_string());
        }
        i += 10 + len;
    }
    None
}

fn http_myip() -> Option<String> {
    use std::io::{Read, Write};
    let addr = std::net::ToSocketAddrs::to_socket_addrs(&("api.ipify.org", 80))
        .ok()?
        .next()?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    s.write_all(b"GET / HTTP/1.1\r\nHost: api.ipify.org\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut body = String::new();
    s.take(4096).read_to_string(&mut body).ok()?;
    let ip = body.split("\r\n\r\n").nth(1)?.trim();
    ip.parse::<std::net::IpAddr>().ok().map(|a| a.to_string())
}

/// "3.1 GB", "512 MB".
pub fn bytes(n: u64) -> String {
    let gb = n as f64 / 1_073_741_824.0;
    if gb >= 100.0 {
        format!("{gb:.0} GB")
    } else if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", n as f64 / 1_048_576.0)
    }
}

#[cfg(windows)]
mod platform {
    use super::Snapshot;
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows_sys::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT_COUNTERVALUE,
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_LARGE,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    type H = *mut core::ffi::c_void;
    const NONE: H = std::ptr::null_mut();
    /// Lets a total above 100 through (several engines added up).
    const PDH_FMT_NOCAP100: u32 = 0x0000_8000;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn ft(f: &FILETIME) -> u64 {
        ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
    }

    pub struct Sampler {
        last_cpu: Option<(u64, u64)>,
        query: H,
        gpu: H,
        gpu_mem: H,
        disks: HashMap<String, H>,
        gpu_total: Option<u64>,
    }

    impl Sampler {
        pub fn new() -> Self {
            let mut query = NONE;
            let mut gpu = NONE;
            let mut gpu_mem = NONE;
            unsafe {
                if PdhOpenQueryW(std::ptr::null(), 0, &mut query) == 0 {
                    let p = wide(r"\GPU Engine(*engtype_3D)\Utilization Percentage");
                    if PdhAddEnglishCounterW(query, p.as_ptr(), 0, &mut gpu) != 0 {
                        gpu = NONE;
                    }
                    let p = wide(r"\GPU Adapter Memory(*)\Dedicated Usage");
                    if PdhAddEnglishCounterW(query, p.as_ptr(), 0, &mut gpu_mem) != 0 {
                        gpu_mem = NONE;
                    }
                    PdhCollectQueryData(query);
                } else {
                    query = NONE;
                }
            }
            Self {
                last_cpu: None,
                query,
                gpu,
                gpu_mem,
                disks: HashMap::new(),
                gpu_total: gpu_total(),
            }
        }

        fn add_disk(&mut self, drive: &str) {
            if self.query.is_null() || self.disks.contains_key(drive) {
                return;
            }
            let mut c = NONE;
            let p = wide(&format!(r"\LogicalDisk({drive})\% Idle Time"));
            unsafe {
                if PdhAddEnglishCounterW(self.query, p.as_ptr(), 0, &mut c) == 0 {
                    self.disks.insert(drive.to_string(), c);
                }
            }
        }

        fn array_sum(&self, counter: H, format: u32) -> Option<f64> {
            if counter.is_null() {
                return None;
            }
            unsafe {
                let mut size = 0u32;
                let mut count = 0u32;
                PdhGetFormattedCounterArrayW(
                    counter,
                    format,
                    &mut size,
                    &mut count,
                    std::ptr::null_mut(),
                );
                if size == 0 {
                    return None;
                }
                let mut buf = vec![0u8; size as usize];
                let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, items) != 0
                {
                    return None;
                }
                let mut sum = 0.0;
                for i in 0..count as usize {
                    let v = &(*items.add(i)).FmtValue;
                    sum += if format & PDH_FMT_LARGE != 0 {
                        v.Anonymous.largeValue as f64
                    } else {
                        v.Anonymous.doubleValue
                    };
                }
                Some(sum)
            }
        }

        pub fn read(&mut self, drives: &[String]) -> Snapshot {
            for d in drives {
                self.add_disk(d);
            }
            let mut s = Snapshot::default();
            unsafe {
                let (mut idle, mut kernel, mut user) =
                    std::mem::zeroed::<(FILETIME, FILETIME, FILETIME)>();
                if GetSystemTimes(&mut idle, &mut kernel, &mut user) != 0 {
                    let busy_total = (ft(&kernel) + ft(&user), ft(&idle));
                    if let Some((t0, i0)) = self.last_cpu {
                        let total = busy_total.0.saturating_sub(t0);
                        let idle = busy_total.1.saturating_sub(i0);
                        if total > 0 {
                            s.cpu = Some(100.0 * (1.0 - idle as f32 / total as f32));
                        }
                    }
                    self.last_cpu = Some(busy_total);
                }
                let mut m: MEMORYSTATUSEX = std::mem::zeroed();
                m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
                if GlobalMemoryStatusEx(&mut m) != 0 {
                    s.memory = Some((m.ullTotalPhys - m.ullAvailPhys, m.ullTotalPhys));
                }
                for d in drives {
                    let root = wide(&format!("{d}\\"));
                    let (mut free, mut total) = (0u64, 0u64);
                    if GetDiskFreeSpaceExW(
                        root.as_ptr(),
                        std::ptr::null_mut(),
                        &mut total,
                        &mut free,
                    ) != 0
                        && total > 0
                    {
                        s.disk_space.insert(d.clone(), (total - free, total));
                    }
                }
                if !self.query.is_null() && PdhCollectQueryData(self.query) == 0 {
                    s.gpu = self
                        .array_sum(self.gpu, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100)
                        .map(|v| v.clamp(0.0, 100.0) as f32);
                    s.gpu_memory = self
                        .array_sum(self.gpu_mem, PDH_FMT_LARGE)
                        .map(|v| (v as u64, self.gpu_total));
                    for (d, c) in &self.disks {
                        let mut v: PDH_FMT_COUNTERVALUE = std::mem::zeroed();
                        if PdhGetFormattedCounterValue(
                            *c,
                            PDH_FMT_DOUBLE,
                            std::ptr::null_mut(),
                            &mut v,
                        ) == 0
                        {
                            let idle = v.Anonymous.doubleValue.clamp(0.0, 100.0);
                            s.disk_busy.insert(d.clone(), (100.0 - idle) as f32);
                        }
                    }
                }
            }
            s
        }
    }

    /// The largest graphics card memory Windows has on record.
    fn gpu_total() -> Option<u64> {
        use windows_sys::Win32::System::Registry::{
            RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_QWORD,
        };
        let mut best = None;
        for i in 0..10 {
            let key = wide(&format!(
                r"SYSTEM\CurrentControlSet\Control\Class\{{4d36e968-e325-11ce-bfc1-08002be10318}}\{i:04}"
            ));
            let name = wide("HardwareInformation.qwMemorySize");
            let mut v = 0u64;
            let mut size = 8u32;
            let ok = unsafe {
                RegGetValueW(
                    HKEY_LOCAL_MACHINE,
                    key.as_ptr(),
                    name.as_ptr(),
                    RRF_RT_QWORD,
                    std::ptr::null_mut(),
                    &mut v as *mut u64 as *mut _,
                    &mut size,
                ) == 0
            };
            if ok && v > 0 {
                best = Some(best.map_or(v, |b: u64| b.max(v)));
            }
        }
        best
    }
}

#[cfg(not(windows))]
mod platform {
    //! Linux readings, enough to try the keys out while developing.
    use super::Snapshot;

    pub struct Sampler {
        last_cpu: Option<(u64, u64)>,
    }

    impl Sampler {
        pub fn new() -> Self {
            Self { last_cpu: None }
        }

        pub fn read(&mut self, _drives: &[String]) -> Snapshot {
            let mut s = Snapshot::default();
            if let Ok(t) = std::fs::read_to_string("/proc/stat") {
                let n: Vec<u64> = t
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .skip(1)
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if n.len() >= 4 {
                    let idle = n[3] + n.get(4).copied().unwrap_or(0);
                    let total: u64 = n.iter().sum();
                    if let Some((t0, i0)) = self.last_cpu {
                        let dt = total.saturating_sub(t0);
                        if dt > 0 {
                            s.cpu =
                                Some(100.0 * (1.0 - idle.saturating_sub(i0) as f32 / dt as f32));
                        }
                    }
                    self.last_cpu = Some((total, idle));
                }
            }
            if let Ok(t) = std::fs::read_to_string("/proc/meminfo") {
                let get = |k: &str| {
                    t.lines()
                        .find(|l| l.starts_with(k))
                        .and_then(|l| l.split_whitespace().nth(1))
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|kb| kb * 1024)
                };
                if let (Some(total), Some(avail)) = (get("MemTotal:"), get("MemAvailable:")) {
                    s.memory = Some((total - avail, total));
                }
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_a_record() {
        // Reply to id 0x1234: one question (myip.opendns.com A IN), one answer.
        let mut r = vec![0x12, 0x34, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
        for l in ["myip", "opendns", "com"] {
            r.push(l.len() as u8);
            r.extend_from_slice(l.as_bytes());
        }
        r.extend_from_slice(&[0, 0, 1, 0, 1]);
        r.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4, 203, 0, 113, 7]);
        assert_eq!(parse_a_answer(&r, 0x1234).as_deref(), Some("203.0.113.7"));
        assert_eq!(parse_a_answer(&r, 0x9999), None);
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(bytes(512 * 1_048_576), "512 MB");
        assert_eq!(bytes(3 * 1_073_741_824 + 107_374_182), "3.1 GB");
        assert_eq!(bytes(512 * 1_073_741_824), "512 GB");
    }
}
