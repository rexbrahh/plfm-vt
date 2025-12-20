//! Logs API endpoints.
//!
//! Provides query and streaming endpoints backed by stored workload logs.

use std::{collections::VecDeque, convert::Infallible, time::Duration};

use axum::{
    extract::{Path, Query, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use chrono::{DateTime, Utc};
use futures_util::stream::unfold;
use plfm_id::{AppId, EnvId, OrgId};
use serde::{Deserialize, Serialize};
use sqlx::QueryBuilder;
use tokio::time::sleep;

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::state::AppState;

const MAX_TAIL_LINES: i64 = 10_000;
const DEFAULT_TAIL_LINES: i64 = 200;
const STREAM_BATCH_LIMIT: i64 = 200;
const STREAM_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Query parameters for log queries.
#[derive(Debug, Deserialize)]
pub struct QueryLogsParams {
    pub process_type: Option<String>,
    pub instance_id: Option<String>,
    /// RFC3339 timestamp (inclusive).
    pub since: Option<String>,
    /// RFC3339 timestamp (inclusive).
    pub until: Option<String>,
    pub tail_lines: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct LogLine {
    pub ts: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    pub line: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub items: Vec<LogLine>,
}

#[derive(Debug, Clone)]
struct LogQueryFilters {
    process_type: Option<String>,
    instance_id: Option<String>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
}

#[derive(Debug)]
struct LogRow {
    log_id: i64,
    ts: DateTime<Utc>,
    instance_id: String,
    process_type: String,
    stream: String,
    line: String,
    truncated: bool,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for LogRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            log_id: row.try_get("log_id")?,
            ts: row.try_get("ts")?,
            instance_id: row.try_get("instance_id")?,
            process_type: row.try_get("process_type")?,
            stream: row.try_get("stream")?,
            line: row.try_get("line")?,
            truncated: row.try_get("truncated")?,
        })
    }
}

struct LogStreamState {
    state: AppState,
    org_id: OrgId,
    app_id: AppId,
    env_id: EnvId,
    filters: LogQueryFilters,
    tail_lines: i64,
    last_id: i64,
    buffer: VecDeque<LogRow>,
    initialized: bool,
}

/// Query logs (bounded window).
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/logs
pub async fn query_logs(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Query(query): Query<QueryLogsParams>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;
    let env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let since = parse_rfc3339(query.since.as_deref(), "since", &request_id)?;
    let until = parse_rfc3339(query.until.as_deref(), "until", &request_id)?;
    if let (Some(since), Some(until)) = (since, until) {
        if since > until {
            return Err(ApiError::bad_request(
                "invalid_time_range",
                "'since' must be before 'until'",
            )
            .with_request_id(request_id));
        }
    }

    let tail_lines = query
        .tail_lines
        .unwrap_or(DEFAULT_TAIL_LINES)
        .clamp(1, MAX_TAIL_LINES);

    let filters = LogQueryFilters {
        process_type: query.process_type.clone(),
        instance_id: query.instance_id.clone(),
        since,
        until,
    };

    let mut rows = fetch_log_rows(
        &state,
        &org_id,
        &app_id,
        &env_id,
        &filters,
        None,
        tail_lines,
        false,
        &request_id,
    )
    .await?;

    rows.reverse();

    let items = rows
        .into_iter()
        .map(|row| LogLine {
            ts: row.ts,
            instance_id: Some(row.instance_id),
            process_type: Some(row.process_type),
            stream: Some(row.stream),
            line: row.line,
            truncated: Some(row.truncated),
        })
        .collect();

    Ok(Json(LogsResponse { items }))
}

/// Stream logs (server-sent events).
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/logs/stream
pub async fn stream_logs(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Query(query): Query<QueryLogsParams>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;
    let env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let since = parse_rfc3339(query.since.as_deref(), "since", &request_id)?;
    let until = parse_rfc3339(query.until.as_deref(), "until", &request_id)?;

    let tail_lines = query
        .tail_lines
        .unwrap_or(DEFAULT_TAIL_LINES)
        .clamp(0, MAX_TAIL_LINES);

    let filters = LogQueryFilters {
        process_type: query.process_type.clone(),
        instance_id: query.instance_id.clone(),
        since,
        until,
    };

    let stream_state = LogStreamState {
        state: state.clone(),
        org_id,
        app_id,
        env_id,
        filters,
        tail_lines,
        last_id: 0,
        buffer: VecDeque::new(),
        initialized: false,
    };

    let stream = unfold(stream_state, move |mut st| async move {
        loop {
            if let Some(row) = st.buffer.pop_front() {
                let log_line = LogLine {
                    ts: row.ts,
                    instance_id: Some(row.instance_id),
                    process_type: Some(row.process_type),
                    stream: Some(row.stream),
                    line: row.line,
                    truncated: Some(row.truncated),
                };

                let data = match serde_json::to_string(&log_line) {
                    Ok(data) => data,
                    Err(e) => {
                        tracing::error!(error = ?e, "Failed to serialize log line");
                        continue;
                    }
                };

                let event = Event::default().event("log").data(data);
                return Some((Ok::<Event, Infallible>(event), st));
            }

            if !st.initialized {
                st.initialized = true;
                if st.tail_lines > 0 {
                    match fetch_log_rows(
                        &st.state,
                        &st.org_id,
                        &st.app_id,
                        &st.env_id,
                        &st.filters,
                        None,
                        st.tail_lines,
                        false,
                        "stream_logs",
                    )
                    .await
                    {
                        Ok(mut rows) => {
                            rows.reverse();
                            if let Some(last) = rows.last() {
                                st.last_id = last.log_id;
                            }
                            st.buffer = VecDeque::from(rows);
                            continue;
                        }
                        Err(e) => {
                            tracing::error!(error = ?e, "Failed to fetch initial log batch");
                        }
                    }
                }
            }

            if let Some(until) = st.filters.until.as_ref() {
                if Utc::now() > *until {
                    return None;
                }
            }

            match fetch_log_rows(
                &st.state,
                &st.org_id,
                &st.app_id,
                &st.env_id,
                &st.filters,
                Some(st.last_id),
                STREAM_BATCH_LIMIT,
                true,
                "stream_logs",
            )
            .await
            {
                Ok(rows) => {
                    if rows.is_empty() {
                        sleep(STREAM_POLL_INTERVAL).await;
                        continue;
                    }

                    if let Some(last) = rows.last() {
                        st.last_id = last.log_id;
                    }
                    st.buffer = VecDeque::from(rows);
                }
                Err(e) => {
                    tracing::error!(error = ?e, "Failed to fetch log batch");
                    sleep(STREAM_POLL_INTERVAL).await;
                }
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

fn parse_rfc3339(
    value: Option<&str>,
    field: &str,
    request_id: &str,
) -> Result<Option<DateTime<Utc>>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };

    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        ApiError::bad_request(
            format!("invalid_{field}"),
            format!("Invalid '{field}' timestamp (expected RFC3339)"),
        )
        .with_request_id(request_id.to_string())
    })?;

    Ok(Some(parsed.with_timezone(&Utc)))
}

#[allow(clippy::too_many_arguments)]
async fn fetch_log_rows(
    state: &AppState,
    org_id: &OrgId,
    app_id: &AppId,
    env_id: &EnvId,
    filters: &LogQueryFilters,
    min_log_id: Option<i64>,
    limit: i64,
    order_asc: bool,
    request_id: &str,
) -> Result<Vec<LogRow>, ApiError> {
    let mut builder = QueryBuilder::new(
        "SELECT log_id, ts, instance_id, process_type, stream, line, truncated \
         FROM workload_logs WHERE org_id = ",
    );
    builder.push_bind(org_id.to_string());
    builder.push(" AND app_id = ");
    builder.push_bind(app_id.to_string());
    builder.push(" AND env_id = ");
    builder.push_bind(env_id.to_string());

    if let Some(process_type) = filters.process_type.as_ref() {
        builder.push(" AND process_type = ");
        builder.push_bind(process_type);
    }

    if let Some(instance_id) = filters.instance_id.as_ref() {
        builder.push(" AND instance_id = ");
        builder.push_bind(instance_id);
    }

    if let Some(min_log_id) = min_log_id {
        builder.push(" AND log_id > ");
        builder.push_bind(min_log_id);
    }

    if let Some(since) = filters.since.as_ref() {
        builder.push(" AND ts >= ");
        builder.push_bind(*since);
    }

    if let Some(until) = filters.until.as_ref() {
        builder.push(" AND ts <= ");
        builder.push_bind(*until);
    }

    if order_asc {
        builder.push(" ORDER BY log_id ASC");
    } else {
        builder.push(" ORDER BY log_id DESC");
    }

    builder.push(" LIMIT ");
    builder.push_bind(limit);

    builder
        .build_query_as::<LogRow>()
        .fetch_all(state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, request_id = %request_id, "Failed to query logs");
            ApiError::internal("internal_error", "Failed to query logs")
                .with_request_id(request_id.to_string())
        })
}
