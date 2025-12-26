#[derive(Debug, Clone)]
pub struct SystemResources {
    pub cpu_cores: i32,
    pub total_memory_bytes: i64,
    pub available_memory_bytes: i64,
}

impl SystemResources {
    pub fn measure() -> Self {
        let cpu_cores = get_cpu_count();
        let (total_memory, available_memory) = get_memory_info();

        Self {
            cpu_cores,
            total_memory_bytes: total_memory,
            available_memory_bytes: available_memory,
        }
    }
}

fn get_cpu_count() -> i32 {
    #[cfg(unix)]
    {
        let count = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
        if count > 0 {
            return count as i32;
        }
    }
    
    std::thread::available_parallelism()
        .map(|p| p.get() as i32)
        .unwrap_or(1)
}

#[cfg(target_os = "linux")]
fn get_memory_info() -> (i64, i64) {
    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        return parse_meminfo(&meminfo);
    }

    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let total_pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let avail_pages = unsafe { libc::sysconf(libc::_SC_AVPHYS_PAGES) };

    if page_size > 0 && total_pages > 0 {
        let total = (page_size * total_pages) as i64;
        let avail = if avail_pages > 0 {
            (page_size * avail_pages) as i64
        } else {
            total
        };
        return (total, avail);
    }

    (16 * 1024 * 1024 * 1024, 8 * 1024 * 1024 * 1024)
}

#[cfg(not(target_os = "linux"))]
fn get_memory_info() -> (i64, i64) {
    #[cfg(unix)]
    {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let total_pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };

        if page_size > 0 && total_pages > 0 {
            let total = (page_size * total_pages) as i64;
            return (total, total / 2);
        }
    }

    (16 * 1024 * 1024 * 1024, 8 * 1024 * 1024 * 1024)
}

#[cfg(target_os = "linux")]
fn parse_meminfo(content: &str) -> (i64, i64) {
    let mut total: i64 = 0;
    let mut available: i64 = 0;
    let mut free: i64 = 0;
    let mut buffers: i64 = 0;
    let mut cached: i64 = 0;

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            const KB_TO_BYTES: i64 = 1024;
            let value: i64 = parts[1].parse().unwrap_or(0) * KB_TO_BYTES;
            match parts[0] {
                "MemTotal:" => total = value,
                "MemAvailable:" => available = value,
                "MemFree:" => free = value,
                "Buffers:" => buffers = value,
                "Cached:" => cached = value,
                _ => {}
            }
        }
    }

    if available == 0 {
        available = free + buffers + cached;
    }

    (total, available)
}

#[cfg(not(target_os = "linux"))]
fn parse_meminfo(_content: &str) -> (i64, i64) {
    (16 * 1024 * 1024 * 1024, 8 * 1024 * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_measure_resources() {
        let resources = SystemResources::measure();
        assert!(resources.cpu_cores > 0);
        assert!(resources.total_memory_bytes > 0);
        assert!(resources.available_memory_bytes > 0);
        assert!(resources.available_memory_bytes <= resources.total_memory_bytes);
    }

    #[test]
    fn test_get_cpu_count() {
        let count = get_cpu_count();
        assert!(count >= 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_meminfo() {
        let sample = r#"MemTotal:       16384000 kB
MemFree:         1234567 kB
MemAvailable:    8000000 kB
Buffers:          123456 kB
Cached:          2345678 kB
"#;
        let (total, available) = parse_meminfo(sample);
        assert_eq!(total, 16384000 * 1024);
        assert_eq!(available, 8000000 * 1024);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_meminfo_no_available() {
        let sample = r#"MemTotal:       16384000 kB
MemFree:         1000000 kB
Buffers:          500000 kB
Cached:          2000000 kB
"#;
        let (total, available) = parse_meminfo(sample);
        assert_eq!(total, 16384000 * 1024);
        assert_eq!(available, (1000000 + 500000 + 2000000) * 1024);
    }
}
