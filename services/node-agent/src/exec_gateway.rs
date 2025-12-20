//! Exec gateway server for node-agent.
//!
//! Accepts connections from the control plane and proxies exec streams to guest-init.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream as StdTcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{info, warn};
use vsock::{VsockAddr, VsockStream};

use crate::exec::{frame_type, ExecRequest};
use crate::instance::InstanceManager;

const FRAME_INIT: u8 = 0x20;

#[derive(Debug, Serialize, Deserialize)]
struct ExecConnectInit {
    session_id: String,
    instance_id: String,
    command: Vec<String>,
    tty: bool,
    cols: u16,
    rows: u16,
    env: HashMap<String, String>,
    stdin: bool,
}

#[derive(Debug, Serialize)]
struct ExitPayload {
    #[serde(rename = "type")]
    msg_type: &'static str,
    exit_code: i32,
    reason: String,
}

/// Exec gateway server.
pub struct ExecGateway {
    listen_addr: SocketAddr,
    instance_manager: Arc<InstanceManager>,
}

impl ExecGateway {
    pub fn new(listen_addr: SocketAddr, instance_manager: Arc<InstanceManager>) -> Self {
        Self {
            listen_addr,
            instance_manager,
        }
    }

    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(self.listen_addr).await?;
        info!(addr = %self.listen_addr, "Exec gateway listening");

        loop {
            let (stream, peer) = listener.accept().await?;
            let instance_manager = Arc::clone(&self.instance_manager);
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, peer, instance_manager).await {
                    warn!(error = %e, peer = %peer, "Exec gateway connection failed");
                }
            });
        }
    }
}

async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    peer: SocketAddr,
    instance_manager: Arc<InstanceManager>,
) -> Result<()> {
    let init_frame = read_framed(&mut stream).await?;
    let Some(init_frame) = init_frame else {
        return Ok(());
    };

    if init_frame.is_empty() || init_frame[0] != FRAME_INIT {
        warn!(peer = %peer, "Exec gateway received invalid init frame");
        return Ok(());
    }

    let init: ExecConnectInit = serde_json::from_slice(&init_frame[1..])?;
    info!(session_id = %init.session_id, instance_id = %init.instance_id, "Exec session init received");

    let guest_cid = match instance_manager
        .guest_cid_for_instance(&init.instance_id)
        .await
    {
        Some(cid) => cid,
        None => {
            send_exit_frame(&mut stream, 128, "instance_not_ready").await?;
            return Ok(());
        }
    };

    let std_stream = stream.into_std()?;
    std_stream.set_nonblocking(false)?;

    tokio::task::spawn_blocking(move || run_exec_session(std_stream, guest_cid, init)).await??;

    Ok(())
}

fn run_exec_session(
    mut tcp_stream: StdTcpStream,
    guest_cid: u32,
    init: ExecConnectInit,
) -> Result<()> {
    let addr = VsockAddr::new(guest_cid, crate::exec::EXEC_PORT);
    let mut vsock = VsockStream::connect(&addr)
        .map_err(|e| anyhow!("Failed to connect to guest exec service: {e}"))?;

    let request = ExecRequest {
        command: init.command,
        env: init.env,
        tty: init.tty,
        cols: init.cols,
        rows: init.rows,
        stdin: init.stdin,
    };

    let request_json = serde_json::to_string(&request)?;
    vsock.write_all(request_json.as_bytes())?;
    vsock.write_all(b"\n")?;
    vsock.flush()?;

    let mut vsock_reader = vsock.try_clone()?;
    let mut vsock_writer = vsock;

    let mut tcp_reader = tcp_stream.try_clone()?;
    let mut tcp_writer = tcp_stream.try_clone()?;

    let done = Arc::new(AtomicBool::new(false));
    let exit_sent = Arc::new(AtomicBool::new(false));

    let done_reader = Arc::clone(&done);
    let exit_sent_reader = Arc::clone(&exit_sent);

    let reader_thread = std::thread::spawn(move || -> Result<()> {
        let mut buf = [0u8; 4096];
        loop {
            let n = match vsock_reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    warn!(error = %e, "Vsock read error");
                    break;
                }
            };

            let frame = &buf[..n];
            if frame.is_empty() {
                continue;
            }

            if frame[0] == frame_type::EXIT {
                exit_sent_reader.store(true, Ordering::SeqCst);
            }

            write_framed_blocking(&mut tcp_writer, frame)?;

            if frame[0] == frame_type::EXIT {
                break;
            }
        }

        let _ = tcp_writer.shutdown(Shutdown::Both);
        done_reader.store(true, Ordering::SeqCst);
        Ok(())
    });

    tcp_stream.set_read_timeout(Some(Duration::from_millis(200)))?;

    loop {
        if done.load(Ordering::SeqCst) {
            break;
        }

        match read_framed_blocking(&mut tcp_reader) {
            Ok(Some(frame)) => {
                vsock_writer.write_all(&frame)?;
                vsock_writer.flush()?;
            }
            Ok(None) => break,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::WouldBlock
                    && e.kind() != std::io::ErrorKind::TimedOut
                {
                    warn!(error = %e, "TCP read error");
                    break;
                }
            }
        }
    }

    if !exit_sent.load(Ordering::SeqCst) {
        let payload = ExitPayload {
            msg_type: "exit",
            exit_code: 128,
            reason: "client_disconnect".to_string(),
        };
        let payload = serde_json::to_vec(&payload)?;
        let mut frame = Vec::with_capacity(1 + payload.len());
        frame.push(frame_type::EXIT);
        frame.extend_from_slice(&payload);
        let _ = write_framed_blocking(&mut tcp_stream, &frame);
    }

    let _ = vsock_writer.shutdown(Shutdown::Both);
    let _ = tcp_stream.shutdown(Shutdown::Both);

    let _ = reader_thread.join();

    Ok(())
}

async fn read_framed(stream: &mut tokio::net::TcpStream) -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    let mut frame = vec![0u8; len];
    stream.read_exact(&mut frame).await?;
    Ok(Some(frame))
}

async fn send_exit_frame(
    stream: &mut tokio::net::TcpStream,
    exit_code: i32,
    reason: &str,
) -> Result<()> {
    let payload = ExitPayload {
        msg_type: "exit",
        exit_code,
        reason: reason.to_string(),
    };
    let payload = serde_json::to_vec(&payload)?;
    let mut frame = Vec::with_capacity(1 + payload.len());
    frame.push(frame_type::EXIT);
    frame.extend_from_slice(&payload);
    let len = frame.len() as u32;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&frame).await?;
    stream.flush().await?;
    Ok(())
}

fn read_framed_blocking(stream: &mut StdTcpStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    let mut frame = vec![0u8; len];
    stream.read_exact(&mut frame)?;
    Ok(Some(frame))
}

fn write_framed_blocking(stream: &mut StdTcpStream, frame: &[u8]) -> Result<()> {
    let len = frame.len() as u32;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(frame)?;
    stream.flush()?;
    Ok(())
}
