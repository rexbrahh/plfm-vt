use std::net::{Ipv6Addr, SocketAddrV6};
use std::time::Duration;

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::config::HealthConfig;
use crate::handshake;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
}

pub async fn run_health_checks(config: HealthConfig) -> Result<()> {
    let check_timeout = Duration::from_secs(config.timeout_seconds as u64);
    let interval = Duration::from_secs(config.interval_seconds as u64);
    let grace_period = Duration::from_secs(config.grace_period_seconds as u64);

    info!(
        health_type = %config.health_type,
        port = config.port,
        path = ?config.path,
        interval_seconds = config.interval_seconds,
        grace_period_seconds = config.grace_period_seconds,
        success_threshold = config.success_threshold,
        failure_threshold = config.failure_threshold,
        "starting health check loop"
    );

    tokio::time::sleep(grace_period).await;
    debug!("grace period elapsed, beginning health checks");

    let mut consecutive_successes = 0;
    let mut consecutive_failures = 0;
    let mut is_ready = false;

    loop {
        let result = match config.health_type.as_str() {
            "tcp" => check_tcp(config.port, check_timeout).await,
            "http" => check_http(config.port, config.path.as_deref(), check_timeout).await,
            other => {
                warn!(health_type = %other, "unknown health check type, defaulting to tcp");
                check_tcp(config.port, check_timeout).await
            }
        };

        match result {
            HealthStatus::Healthy => {
                consecutive_successes += 1;
                consecutive_failures = 0;
                debug!(consecutive_successes, "health check passed");

                if !is_ready && consecutive_successes >= config.success_threshold {
                    info!("health checks passed, reporting ready");
                    handshake::report_status("ready").await?;
                    is_ready = true;
                }
            }
            HealthStatus::Unhealthy => {
                consecutive_failures += 1;
                consecutive_successes = 0;
                debug!(consecutive_failures, "health check failed");

                if is_ready && consecutive_failures >= config.failure_threshold {
                    warn!("health checks failing, reporting unhealthy");
                    handshake::report_status("unhealthy").await?;
                    is_ready = false;
                }
            }
        }

        tokio::time::sleep(interval).await;
    }
}

async fn check_tcp(port: i32, check_timeout: Duration) -> HealthStatus {
    let addr = SocketAddrV6::new(Ipv6Addr::LOCALHOST, port as u16, 0, 0);

    match timeout(check_timeout, TcpStream::connect(addr)).await {
        Ok(Ok(_stream)) => {
            debug!(port, "tcp health check succeeded");
            HealthStatus::Healthy
        }
        Ok(Err(e)) => {
            debug!(port, error = %e, "tcp health check failed: connection error");
            HealthStatus::Unhealthy
        }
        Err(_) => {
            debug!(port, "tcp health check failed: timeout");
            HealthStatus::Unhealthy
        }
    }
}

async fn check_http(port: i32, path: Option<&str>, check_timeout: Duration) -> HealthStatus {
    let path = path.unwrap_or("/");
    let addr = SocketAddrV6::new(Ipv6Addr::LOCALHOST, port as u16, 0, 0);

    let connect_result = match timeout(check_timeout, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            debug!(port, path, error = %e, "http health check failed: connection error");
            return HealthStatus::Unhealthy;
        }
        Err(_) => {
            debug!(port, path, "http health check failed: connect timeout");
            return HealthStatus::Unhealthy;
        }
    };

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        path
    );

    let mut stream = connect_result;
    if let Err(e) = stream.write_all(request.as_bytes()).await {
        debug!(port, path, error = %e, "http health check failed: write error");
        return HealthStatus::Unhealthy;
    }

    let mut response = vec![0u8; 1024];
    match timeout(
        check_timeout,
        tokio::io::AsyncReadExt::read(&mut stream, &mut response),
    )
    .await
    {
        Ok(Ok(n)) if n > 0 => {
            let response_str = String::from_utf8_lossy(&response[..n]);
            if let Some(status_line) = response_str.lines().next() {
                if status_line.contains(" 2") {
                    debug!(port, path, status = %status_line, "http health check succeeded");
                    return HealthStatus::Healthy;
                } else {
                    debug!(port, path, status = %status_line, "http health check failed: non-2xx status");
                }
            }
        }
        Ok(Ok(_)) => {
            debug!(port, path, "http health check failed: empty response");
        }
        Ok(Err(e)) => {
            debug!(port, path, error = %e, "http health check failed: read error");
        }
        Err(_) => {
            debug!(port, path, "http health check failed: read timeout");
        }
    }

    HealthStatus::Unhealthy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tcp_check_no_listener() {
        let status = check_tcp(59999, Duration::from_millis(100)).await;
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn test_http_check_no_listener() {
        let status = check_http(59999, Some("/health"), Duration::from_millis(100)).await;
        assert_eq!(status, HealthStatus::Unhealthy);
    }
}
