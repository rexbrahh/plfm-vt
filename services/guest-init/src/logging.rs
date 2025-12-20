//! Logging initialization for guest init.
//!
//! Boot logs are written to a fixed path for diagnostics.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

/// Boot log writer that truncates at max size.
struct BootLogWriter {
    file: File,
    bytes_written: usize,
    max_bytes: usize,
}

impl BootLogWriter {
    fn new(path: &Path, max_bytes: usize) -> io::Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;

        Ok(Self {
            file,
            bytes_written: 0,
            max_bytes,
        })
    }
}

impl Write for BootLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.bytes_written >= self.max_bytes {
            // Drop logs after max size
            return Ok(buf.len());
        }

        let remaining = self.max_bytes - self.bytes_written;
        let to_write = buf.len().min(remaining);
        let written = self.file.write(&buf[..to_write])?;
        self.bytes_written += written;
        Ok(buf.len()) // Pretend we wrote everything
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Thread-safe writer wrapper.
struct SharedWriter(Mutex<BootLogWriter>);

impl Write for &SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for &'static SharedWriter {
    type Writer = &'static SharedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}

/// Maximum boot log size (1 MB).
const MAX_LOG_BYTES: usize = 1024 * 1024;

/// Initialize logging to boot log file.
pub fn init(log_path: &str) -> anyhow::Result<()> {
    // Create the boot log writer
    let writer = BootLogWriter::new(Path::new(log_path), MAX_LOG_BYTES)?;

    // Leak to get 'static lifetime (guest-init runs for lifetime of process)
    let shared: &'static SharedWriter = Box::leak(Box::new(SharedWriter(Mutex::new(writer))));

    // Set up tracing with JSON format
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(shared)
        .with_filter(filter);

    // Also write to stderr for debugging
    let stderr_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_writer(io::stderr);

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(stderr_layer)
        .init();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::tempdir;

    #[test]
    fn test_boot_log_writer_truncates() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        let mut writer = BootLogWriter::new(&path, 100).unwrap();

        // Write more than max
        let data = "x".repeat(200);
        let written = writer.write(data.as_bytes()).unwrap();
        assert_eq!(written, 200); // Reports full write
        writer.flush().unwrap();

        // But file only has 100 bytes
        let mut contents = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents.len(), 100);
    }
}
