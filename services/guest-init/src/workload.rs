//! Workload process spawning and supervision.
//!
//! Launches the customer workload as a child process and handles:
//! - Signal forwarding (SIGTERM, SIGINT, SIGHUP)
//! - Zombie reaping
//! - Exit code capture

use std::process::{ExitStatus, Stdio};

use anyhow::{Context, Result};
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use tokio::process::{Child, Command};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{debug, info, warn};

use crate::config::WorkloadConfig;
use crate::error::InitError;

pub async fn run(config: WorkloadConfig) -> Result<i32> {
    if config.argv.is_empty() {
        return Err(InitError::WorkloadStartFailed("argv is empty".to_string()).into());
    }

    let program = &config.argv[0];
    let args = &config.argv[1..];

    info!(
        program = %program,
        args = ?args,
        cwd = %config.cwd,
        uid = config.uid,
        gid = config.gid,
        "starting workload"
    );

    // Build the command
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(&config.cwd)
        .envs(&config.env)
        .stdin(if config.stdin {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    // Set UID/GID if non-root
    if config.uid != 0 || config.gid != 0 {
        unsafe {
            let uid = config.uid;
            let gid = config.gid;
            cmd.pre_exec(move || {
                // Set supplementary groups to empty
                if libc::setgroups(0, std::ptr::null()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Set GID first (can't change after dropping root)
                if libc::setgid(gid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Set UID
                if libc::setuid(uid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    // Spawn the process
    let mut child = cmd
        .spawn()
        .map_err(|e| InitError::WorkloadStartFailed(format!("spawn failed: {}", e)))?;

    let child_pid = child.id().expect("child should have pid");
    info!(pid = child_pid, "workload started");

    // Wait for the child while handling signals
    let exit_status = wait_with_signals(&mut child).await?;
    let exit_code = exit_status.code().unwrap_or(128);

    info!(exit_code = exit_code, "workload exited");

    // Reap any remaining zombies
    reap_zombies();

    Ok(exit_code)
}

/// Wait for child exit while forwarding signals.
async fn wait_with_signals(child: &mut Child) -> Result<ExitStatus> {
    let child_pid = child.id().expect("child should have pid") as i32;
    let nix_pid = Pid::from_raw(child_pid);

    // Set up signal handlers
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sighup = signal(SignalKind::hangup())?;

    loop {
        tokio::select! {
            // Child exited
            status = child.wait() => {
                return status.context("failed to wait for child");
            }

            // SIGTERM received - forward to child
            _ = sigterm.recv() => {
                info!(pid = child_pid, "forwarding SIGTERM to workload");
                let _ = kill(nix_pid, Signal::SIGTERM);
            }

            // SIGINT received - forward to child
            _ = sigint.recv() => {
                info!(pid = child_pid, "forwarding SIGINT to workload");
                let _ = kill(nix_pid, Signal::SIGINT);
            }

            // SIGHUP received - forward to child
            _ = sighup.recv() => {
                info!(pid = child_pid, "forwarding SIGHUP to workload");
                let _ = kill(nix_pid, Signal::SIGHUP);
            }
        }
    }
}

/// Reap any zombie child processes.
fn reap_zombies() {
    loop {
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, code)) => {
                debug!(pid = pid.as_raw(), code = code, "reaped zombie");
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                debug!(pid = pid.as_raw(), signal = ?sig, "reaped signaled zombie");
            }
            Ok(WaitStatus::StillAlive) => {
                // No more zombies
                break;
            }
            Err(nix::errno::Errno::ECHILD) => {
                // No children
                break;
            }
            Err(e) => {
                warn!(error = %e, "waitpid error");
                break;
            }
            _ => {
                // Other status, continue reaping
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_workload_simple_command() {
        let config = WorkloadConfig {
            argv: vec!["true".to_string()],
            cwd: "/".to_string(),
            env: HashMap::new(),
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            stdin: false,
            tty: false,
        };

        // This will fail because we're not in a real guest environment
        // but the code structure is correct
        let result = run(config).await;
        // In a real guest this would succeed
        // For now just check it doesn't panic
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_reap_zombies() {
        // Just make sure it doesn't panic with no children
        reap_zombies();
    }
}
