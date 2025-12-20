-- Migration: 00010_create_workload_logs
-- Description: Store workload logs for query and tail endpoints

CREATE TABLE IF NOT EXISTS workload_logs (
    log_id BIGSERIAL PRIMARY KEY,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    env_id TEXT NOT NULL,
    process_type TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    ts TIMESTAMPTZ NOT NULL,
    stream TEXT NOT NULL,
    line TEXT NOT NULL,
    truncated BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_workload_logs_org_ts
    ON workload_logs (org_id, ts DESC);

CREATE INDEX IF NOT EXISTS idx_workload_logs_env_ts
    ON workload_logs (env_id, ts DESC);

CREATE INDEX IF NOT EXISTS idx_workload_logs_instance_ts
    ON workload_logs (instance_id, ts DESC);

CREATE INDEX IF NOT EXISTS idx_workload_logs_log_id
    ON workload_logs (log_id);

COMMENT ON TABLE workload_logs IS 'Workload log lines shipped by node agents';
