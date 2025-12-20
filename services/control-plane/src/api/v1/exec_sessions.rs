//! Exec session connect and lookup endpoints.
//!
//! Provides:
//! - GET /v1/exec-sessions/{id} (status)
//! - GET /v1/exec-sessions/{id}/connect (WebSocket proxy)

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::{
    extract::{ws::Message, ws::WebSocket, ws::WebSocketUpgrade, Path, Query, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use plfm_events::{
    event_types, ActorType, AggregateType, ExecSessionConnectedPayload, ExecSessionEndedPayload,
};
use plfm_id::{ExecSessionId, InstanceId, OrgId, RequestId};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{error, warn};

use crate::api::error::ApiError;
use crate::api::tokens;
use crate::db::AppendEvent;
use crate::state::AppState;

const FRAME_INIT: u8 = 0x20;
const FRAME_EXIT: u8 = 0x11;
const DEFAULT_EXEC_COLS: u16 = 80;
const DEFAULT_EXEC_ROWS: u16 = 24;

#[derive(Debug, Deserialize)]
struct ExecConnectQuery {
    token: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExecSessionStatusResponse {
    exec_session_id: String,
    instance_id: String,
    status: String,
    command: Vec<String>,
    tty: bool,
    created_at: DateTime<Utc>,
    connected_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    exit_code: Option<i32>,
    end_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExecConnectInit {
    session_id: String,
    instance_id: String,
    command: Vec<String>,
    tty: bool,
    cols: u16,
    rows: u16,
    env: BTreeMap<String, String>,
    stdin: bool,
}

#[derive(Debug, Deserialize)]
struct ExecSessionRow {
    exec_session_id: String,
    org_id: String,
    instance_id: String,
    requested_command: serde_json::Value,
    tty: bool,
    status: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    connected_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    exit_code: Option<i32>,
    end_reason: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ExecSessionRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            exec_session_id: row.try_get("exec_session_id")?,
            org_id: row.try_get("org_id")?,
            instance_id: row.try_get("instance_id")?,
            requested_command: row.try_get("requested_command")?,
            tty: row.try_get("tty")?,
            status: row.try_get("status")?,
            expires_at: row.try_get("expires_at")?,
            created_at: row.try_get("created_at")?,
            connected_at: row.try_get("connected_at")?,
            ended_at: row.try_get("ended_at")?,
            exit_code: row.try_get("exit_code")?,
            end_reason: row.try_get("end_reason")?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct ExecSessionTokenRow {
    exec_session_id: String,
    expires_at: DateTime<Utc>,
    consumed_at: Option<DateTime<Utc>>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ExecSessionTokenRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            exec_session_id: row.try_get("exec_session_id")?,
            expires_at: row.try_get("expires_at")?,
            consumed_at: row.try_get("consumed_at")?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct InstancePlacementRow {
    node_id: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstancePlacementRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            node_id: row.try_get("node_id")?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct NodeAddressRow {
    public_ipv6: Option<String>,
    public_ipv4: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for NodeAddressRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            public_ipv6: row.try_get("public_ipv6")?,
            public_ipv4: row.try_get("public_ipv4")?,
        })
    }
}

#[derive(Debug, Clone)]
struct ExecEndState {
    exit_code: Option<i32>,
    reason: String,
}

impl ExecEndState {
    fn new(exit_code: Option<i32>, reason: &str) -> Self {
        Self {
            exit_code,
            reason: reason.to_string(),
        }
    }
}

/// Exec session routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{exec_session_id}", get(get_exec_session))
        .route("/{exec_session_id}/connect", get(connect_exec_session))
}

/// Get exec session status.
async fn get_exec_session(
    State(state): State<AppState>,
    Path(exec_session_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let exec_session_id: ExecSessionId = exec_session_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_exec_session_id", "Invalid exec session ID format")
    })?;

    let row = sqlx::query_as::<_, ExecSessionRow>(
        r#"
        SELECT exec_session_id, org_id, instance_id, requested_command, tty, status,
               expires_at, created_at, connected_at, ended_at, exit_code, end_reason
        FROM exec_sessions_view
        WHERE exec_session_id = $1
        "#,
    )
    .bind(exec_session_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = ?e, exec_session_id = %exec_session_id, "Failed to load exec session");
        ApiError::internal("internal_error", "Failed to load exec session")
    })?;

    let Some(row) = row else {
        return Err(ApiError::not_found(
            "exec_session_not_found",
            "Exec session not found",
        ));
    };

    let command: Vec<String> = serde_json::from_value(row.requested_command).map_err(|e| {
        tracing::error!(error = ?e, exec_session_id = %exec_session_id, "Invalid exec command payload");
        ApiError::internal("internal_error", "Failed to load exec session")
    })?;

    Ok(Json(ExecSessionStatusResponse {
        exec_session_id: row.exec_session_id,
        instance_id: row.instance_id,
        status: row.status,
        command,
        tty: row.tty,
        created_at: row.created_at,
        connected_at: row.connected_at,
        ended_at: row.ended_at,
        exit_code: row.exit_code,
        end_reason: row.end_reason,
    }))
}

/// Connect to an exec session and proxy bytes to the node agent.
async fn connect_exec_session(
    State(state): State<AppState>,
    Path(exec_session_id): Path<String>,
    Query(query): Query<ExecConnectQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = header_request_id(&headers);

    let token = query
        .token
        .or_else(|| bearer_token(&headers))
        .ok_or_else(|| {
            ApiError::unauthorized("invalid_token", "Missing exec session token")
                .with_request_id(request_id.clone())
        })?;

    let exec_session_id_typed: ExecSessionId = exec_session_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_exec_session_id", "Invalid exec session ID format")
            .with_request_id(request_id.clone())
    })?;

    validate_and_consume_exec_token(&state, &exec_session_id_typed, &token, &request_id).await?;

    let session = load_exec_session(&state, &exec_session_id_typed, &request_id).await?;

    if session.status != "granted" {
        return Err(ApiError::bad_request(
            "exec_session_not_granted",
            "Exec session is not in granted state",
        )
        .with_request_id(request_id));
    }

    if session.expires_at < Utc::now() {
        return Err(
            ApiError::unauthorized("exec_session_expired", "Exec session has expired")
                .with_request_id(request_id),
        );
    }

    let instance_id: InstanceId = session.instance_id.parse().map_err(|_| {
        ApiError::internal("internal_error", "Invalid instance ID in exec session")
            .with_request_id(request_id.clone())
    })?;
    let org_id: OrgId = session.org_id.parse().map_err(|_| {
        ApiError::internal("internal_error", "Invalid org ID in exec session")
            .with_request_id(request_id.clone())
    })?;

    let placement = load_instance_placement(&state, &instance_id, &request_id).await?;
    let node_addr = load_node_address(&state, &placement.node_id, &request_id).await?;
    let agent_socket = resolve_exec_agent_socket(&node_addr, &request_id)?;

    let command: Vec<String> = serde_json::from_value(session.requested_command).map_err(|e| {
        tracing::error!(error = ?e, request_id = %request_id, "Invalid exec command payload");
        ApiError::internal("internal_error", "Failed to start exec session")
            .with_request_id(request_id.clone())
    })?;

    let init = ExecConnectInit {
        session_id: exec_session_id_typed.to_string(),
        instance_id: instance_id.to_string(),
        command,
        tty: session.tty,
        cols: DEFAULT_EXEC_COLS,
        rows: DEFAULT_EXEC_ROWS,
        env: BTreeMap::new(),
        stdin: true,
    };

    Ok(ws.on_upgrade(move |socket| {
        handle_exec_socket(
            socket,
            state,
            exec_session_id_typed,
            org_id,
            instance_id,
            agent_socket,
            init,
        )
    }))
}

async fn handle_exec_socket(
    client_socket: WebSocket,
    state: AppState,
    exec_session_id: ExecSessionId,
    org_id: OrgId,
    instance_id: InstanceId,
    agent_socket: SocketAddr,
    init: ExecConnectInit,
) {
    let mut agent_stream = match TcpStream::connect(agent_socket).await {
        Ok(stream) => stream,
        Err(e) => {
            error!(error = ?e, exec_session_id = %exec_session_id, "Failed to connect to node agent");
            emit_exec_end(
                &state,
                &exec_session_id,
                &org_id,
                &instance_id,
                None,
                "connect_timeout",
            )
            .await;
            return;
        }
    };

    let init_payload = match serde_json::to_vec(&init) {
        Ok(payload) => payload,
        Err(e) => {
            error!(error = ?e, exec_session_id = %exec_session_id, "Failed to serialize exec init");
            emit_exec_end(
                &state,
                &exec_session_id,
                &org_id,
                &instance_id,
                None,
                "connect_timeout",
            )
            .await;
            return;
        }
    };

    if let Err(e) = write_framed(&mut agent_stream, FRAME_INIT, &init_payload).await {
        error!(error = ?e, exec_session_id = %exec_session_id, "Failed to send exec init to node agent");
        emit_exec_end(
            &state,
            &exec_session_id,
            &org_id,
            &instance_id,
            None,
            "connect_timeout",
        )
        .await;
        return;
    }

    if let Err(e) = emit_exec_connected(&state, &exec_session_id, &org_id, &instance_id).await {
        error!(error = ?e, exec_session_id = %exec_session_id, "Failed to emit exec_session.connected");
    }

    let (mut client_sender, mut client_receiver) = client_socket.split();
    let (mut agent_reader, mut agent_writer) = agent_stream.into_split();

    let end_state = Arc::new(tokio::sync::Mutex::new(None::<ExecEndState>));
    let end_emitted = Arc::new(AtomicBool::new(false));

    let end_state_agent = end_state.clone();
    let state_agent = state.clone();
    let exec_session_id_agent = exec_session_id.clone();
    let org_id_agent = org_id.clone();
    let instance_id_agent = instance_id.clone();
    let end_emitted_agent = end_emitted.clone();

    let to_client = tokio::spawn(async move {
        loop {
            match read_framed(&mut agent_reader).await {
                Ok(Some(frame)) => {
                    if frame.is_empty() {
                        continue;
                    }

                    let frame_type = frame[0];
                    let payload = &frame[1..];

                    if frame_type == FRAME_EXIT {
                        let exit = parse_exit_payload(payload);
                        set_end_state(&end_state_agent, exit).await;
                    }

                    if let Err(e) = client_sender.send(Message::Binary(frame.into())).await {
                        warn!(error = ?e, exec_session_id = %exec_session_id_agent, "Failed to send exec frame to client");
                        break;
                    }

                    if frame_type == FRAME_EXIT {
                        break;
                    }
                }
                Ok(None) => {
                    set_end_state(
                        &end_state_agent,
                        ExecEndState::new(None, "client_disconnect"),
                    )
                    .await;
                    break;
                }
                Err(e) => {
                    warn!(error = ?e, exec_session_id = %exec_session_id_agent, "Failed to read from node agent");
                    set_end_state(
                        &end_state_agent,
                        ExecEndState::new(None, "client_disconnect"),
                    )
                    .await;
                    break;
                }
            }
        }

        emit_exec_end_from_state(
            &state_agent,
            &exec_session_id_agent,
            &org_id_agent,
            &instance_id_agent,
            &end_state_agent,
            &end_emitted_agent,
        )
        .await;
    });

    let end_state_client = end_state.clone();
    let state_client = state.clone();
    let exec_session_id_client = exec_session_id.clone();
    let org_id_client = org_id.clone();
    let instance_id_client = instance_id.clone();
    let end_emitted_client = end_emitted.clone();

    let to_agent = tokio::spawn(async move {
        while let Some(msg) = client_receiver.next().await {
            match msg {
                Ok(Message::Binary(bytes)) => {
                    if bytes.is_empty() {
                        continue;
                    }
                    let frame_type = bytes[0];
                    let payload = &bytes[1..];
                    if let Err(e) = write_framed(&mut agent_writer, frame_type, payload).await {
                        warn!(error = ?e, exec_session_id = %exec_session_id_client, "Failed to send exec frame to node agent");
                        break;
                    }
                }
                Ok(Message::Close(_)) => {
                    set_end_state(
                        &end_state_client,
                        ExecEndState::new(None, "client_disconnect"),
                    )
                    .await;
                    break;
                }
                Ok(Message::Text(_)) | Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                    continue;
                }
                Err(e) => {
                    warn!(error = ?e, exec_session_id = %exec_session_id_client, "WebSocket error");
                    set_end_state(
                        &end_state_client,
                        ExecEndState::new(None, "client_disconnect"),
                    )
                    .await;
                    break;
                }
            }
        }

        emit_exec_end_from_state(
            &state_client,
            &exec_session_id_client,
            &org_id_client,
            &instance_id_client,
            &end_state_client,
            &end_emitted_client,
        )
        .await;
    });

    let _ = tokio::join!(to_client, to_agent);
}

fn header_request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| RequestId::new().to_string())
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get("Authorization")?.to_str().ok()?;
    let token = auth.trim().strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

async fn validate_and_consume_exec_token(
    state: &AppState,
    exec_session_id: &ExecSessionId,
    token: &str,
    request_id: &str,
) -> Result<(), ApiError> {
    let token_hash = tokens::hash_token(token);
    let mut tx = state.db().pool().begin().await.map_err(|e| {
        tracing::error!(error = ?e, request_id = %request_id, "Failed to begin token validation txn");
        ApiError::internal("internal_error", "Failed to validate exec token")
            .with_request_id(request_id.to_string())
    })?;

    let row = sqlx::query_as::<_, ExecSessionTokenRow>(
        r#"
        SELECT exec_session_id, expires_at, consumed_at
        FROM exec_session_tokens
        WHERE token_hash = $1
        FOR UPDATE
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(error = ?e, request_id = %request_id, "Failed to load exec token");
        ApiError::internal("internal_error", "Failed to validate exec token")
            .with_request_id(request_id.to_string())
    })?;

    let Some(row) = row else {
        return Err(
            ApiError::unauthorized("invalid_token", "Invalid exec token")
                .with_request_id(request_id.to_string()),
        );
    };

    if row.exec_session_id != exec_session_id.to_string() {
        return Err(
            ApiError::unauthorized("invalid_token", "Invalid exec token")
                .with_request_id(request_id.to_string()),
        );
    }

    if row.expires_at < Utc::now() {
        return Err(
            ApiError::unauthorized("token_expired", "Exec token has expired")
                .with_request_id(request_id.to_string()),
        );
    }

    if row.consumed_at.is_some() {
        return Err(
            ApiError::unauthorized("token_consumed", "Exec token already used")
                .with_request_id(request_id.to_string()),
        );
    }

    sqlx::query(
        r#"
        UPDATE exec_session_tokens
        SET consumed_at = now()
        WHERE token_hash = $1 AND consumed_at IS NULL
        "#,
    )
    .bind(&token_hash)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(error = ?e, request_id = %request_id, "Failed to consume exec token");
        ApiError::internal("internal_error", "Failed to validate exec token")
            .with_request_id(request_id.to_string())
    })?;

    tx.commit().await.map_err(|e| {
        tracing::error!(error = ?e, request_id = %request_id, "Failed to commit exec token txn");
        ApiError::internal("internal_error", "Failed to validate exec token")
            .with_request_id(request_id.to_string())
    })?;

    Ok(())
}

async fn load_exec_session(
    state: &AppState,
    exec_session_id: &ExecSessionId,
    request_id: &str,
) -> Result<ExecSessionRow, ApiError> {
    let row = sqlx::query_as::<_, ExecSessionRow>(
        r#"
        SELECT exec_session_id, org_id, instance_id, requested_command, tty, status,
               expires_at, created_at, connected_at, ended_at, exit_code, end_reason
        FROM exec_sessions_view
        WHERE exec_session_id = $1
        "#,
    )
    .bind(exec_session_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = ?e, request_id = %request_id, "Failed to load exec session");
        ApiError::internal("internal_error", "Failed to load exec session")
            .with_request_id(request_id.to_string())
    })?;

    row.ok_or_else(|| {
        ApiError::not_found("exec_session_not_found", "Exec session not found")
            .with_request_id(request_id.to_string())
    })
}

async fn load_instance_placement(
    state: &AppState,
    instance_id: &InstanceId,
    request_id: &str,
) -> Result<InstancePlacementRow, ApiError> {
    sqlx::query_as::<_, InstancePlacementRow>(
        r#"
        SELECT node_id
        FROM instances_desired_view
        WHERE instance_id = $1 AND desired_state = 'running'
        "#,
    )
    .bind(instance_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = ?e, request_id = %request_id, "Failed to load instance placement");
        ApiError::internal("internal_error", "Failed to start exec session")
            .with_request_id(request_id.to_string())
    })?
    .ok_or_else(|| {
        ApiError::bad_request("instance_not_running", "Instance is not running")
            .with_request_id(request_id.to_string())
    })
}

async fn load_node_address(
    state: &AppState,
    node_id: &str,
    request_id: &str,
) -> Result<NodeAddressRow, ApiError> {
    sqlx::query_as::<_, NodeAddressRow>(
        r#"
        SELECT host(public_ipv6)::TEXT as public_ipv6,
               host(public_ipv4)::TEXT as public_ipv4
        FROM nodes_view
        WHERE node_id = $1
        "#,
    )
    .bind(node_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = ?e, request_id = %request_id, "Failed to load node address");
        ApiError::internal("internal_error", "Failed to start exec session")
            .with_request_id(request_id.to_string())
    })?
    .ok_or_else(|| {
        ApiError::not_found("node_not_found", "Node not found")
            .with_request_id(request_id.to_string())
    })
}

fn resolve_exec_agent_socket(
    node: &NodeAddressRow,
    request_id: &str,
) -> Result<SocketAddr, ApiError> {
    let port = std::env::var("PLFM_NODE_EXEC_PORT")
        .or_else(|_| std::env::var("GHOST_NODE_EXEC_PORT"))
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(5090);

    let addr = if let Some(ipv6) = node.public_ipv6.as_deref() {
        format!("[{ipv6}]:{port}")
    } else if let Some(ipv4) = node.public_ipv4.as_deref() {
        format!("{ipv4}:{port}")
    } else {
        return Err(
            ApiError::internal("node_address_missing", "Node has no public address")
                .with_request_id(request_id.to_string()),
        );
    };

    addr.parse().map_err(|_| {
        ApiError::internal("node_address_invalid", "Invalid node address")
            .with_request_id(request_id.to_string())
    })
}

async fn write_framed<W: AsyncWrite + Unpin>(
    stream: &mut W,
    frame_type: u8,
    payload: &[u8],
) -> Result<(), ApiError> {
    let mut frame = Vec::with_capacity(1 + payload.len());
    frame.push(frame_type);
    frame.extend_from_slice(payload);
    let len = frame.len() as u32;
    stream.write_all(&len.to_be_bytes()).await.map_err(|e| {
        ApiError::internal("exec_proxy_failed", format!("failed to write frame: {e}"))
    })?;
    stream.write_all(&frame).await.map_err(|e| {
        ApiError::internal("exec_proxy_failed", format!("failed to write frame: {e}"))
    })?;
    Ok(())
}

async fn read_framed<R: AsyncRead + Unpin>(stream: &mut R) -> Result<Option<Vec<u8>>, ApiError> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => {
            return Err(ApiError::internal(
                "exec_proxy_failed",
                format!("failed to read frame length: {e}"),
            ))
        }
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    let mut frame = vec![0u8; len];
    stream.read_exact(&mut frame).await.map_err(|e| {
        ApiError::internal(
            "exec_proxy_failed",
            format!("failed to read frame body: {e}"),
        )
    })?;

    Ok(Some(frame))
}

fn parse_exit_payload(payload: &[u8]) -> ExecEndState {
    #[derive(Deserialize)]
    struct ExitPayload {
        exit_code: i32,
        reason: String,
    }

    match serde_json::from_slice::<ExitPayload>(payload) {
        Ok(parsed) => ExecEndState::new(Some(parsed.exit_code), &parsed.reason),
        Err(_) => ExecEndState::new(None, "exited"),
    }
}

async fn set_end_state(state: &tokio::sync::Mutex<Option<ExecEndState>>, new_state: ExecEndState) {
    let mut guard = state.lock().await;
    if guard.is_none() {
        *guard = Some(new_state);
    }
}

async fn emit_exec_connected(
    state: &AppState,
    exec_session_id: &ExecSessionId,
    org_id: &OrgId,
    instance_id: &InstanceId,
) -> Result<(), ApiError> {
    let event_store = state.db().event_store();
    let current_seq = event_store
        .get_latest_aggregate_seq(&AggregateType::ExecSession, &exec_session_id.to_string())
        .await
        .map_err(|e| ApiError::internal("internal_error", e.to_string()))?
        .unwrap_or(0);

    let payload = ExecSessionConnectedPayload {
        exec_session_id: exec_session_id.clone(),
        org_id: org_id.clone(),
        instance_id: instance_id.clone(),
        connected_at: Utc::now().to_rfc3339(),
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        ApiError::internal(
            "internal_error",
            format!("Failed to serialize payload: {e}"),
        )
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::ExecSession,
        aggregate_id: exec_session_id.to_string(),
        aggregate_seq: current_seq + 1,
        event_type: event_types::EXEC_SESSION_CONNECTED.to_string(),
        event_version: 1,
        actor_type: ActorType::System,
        actor_id: "exec_gateway".to_string(),
        org_id: Some(org_id.clone()),
        request_id: RequestId::new().to_string(),
        idempotency_key: None,
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload,
    };

    event_store.append(event).await.map_err(|e| {
        ApiError::internal("internal_error", format!("Failed to append event: {e}"))
    })?;

    Ok(())
}

async fn emit_exec_end_from_state(
    state: &AppState,
    exec_session_id: &ExecSessionId,
    org_id: &OrgId,
    instance_id: &InstanceId,
    end_state: &tokio::sync::Mutex<Option<ExecEndState>>,
    emitted: &AtomicBool,
) {
    if emitted.swap(true, Ordering::SeqCst) {
        return;
    }

    let final_state = end_state
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| ExecEndState::new(None, "client_disconnect"));

    emit_exec_end(
        state,
        exec_session_id,
        org_id,
        instance_id,
        final_state.exit_code,
        &final_state.reason,
    )
    .await;
}

async fn emit_exec_end(
    state: &AppState,
    exec_session_id: &ExecSessionId,
    org_id: &OrgId,
    instance_id: &InstanceId,
    exit_code: Option<i32>,
    reason: &str,
) {
    let event_store = state.db().event_store();
    let current_seq = event_store
        .get_latest_aggregate_seq(&AggregateType::ExecSession, &exec_session_id.to_string())
        .await
        .unwrap_or(None)
        .unwrap_or(0);

    let payload = ExecSessionEndedPayload {
        exec_session_id: exec_session_id.clone(),
        org_id: org_id.clone(),
        instance_id: instance_id.clone(),
        ended_at: Utc::now().to_rfc3339(),
        exit_code,
        end_reason: Some(reason.to_string()),
    };

    let payload = match serde_json::to_value(&payload) {
        Ok(payload) => payload,
        Err(e) => {
            error!(error = ?e, exec_session_id = %exec_session_id, "Failed to serialize exec end payload");
            return;
        }
    };

    let event = AppendEvent {
        aggregate_type: AggregateType::ExecSession,
        aggregate_id: exec_session_id.to_string(),
        aggregate_seq: current_seq + 1,
        event_type: event_types::EXEC_SESSION_ENDED.to_string(),
        event_version: 1,
        actor_type: ActorType::System,
        actor_id: "exec_gateway".to_string(),
        org_id: Some(org_id.clone()),
        request_id: RequestId::new().to_string(),
        idempotency_key: None,
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload,
    };

    if let Err(e) = event_store.append(event).await {
        error!(error = ?e, exec_session_id = %exec_session_id, "Failed to append exec_session.ended");
    }
}
