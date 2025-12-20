-- Migration: 00007_create_secret_material
-- Description: Store encrypted secret material and secret version metadata

--------------------------------------------------------------------------------
-- secret_material
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS secret_material (
    material_id TEXT PRIMARY KEY,
    cipher TEXT NOT NULL,
    nonce BYTEA NOT NULL,
    ciphertext BYTEA NOT NULL,
    master_key_id TEXT NOT NULL,
    wrapped_data_key BYTEA NOT NULL,
    wrapped_data_key_nonce BYTEA NOT NULL,
    plaintext_size_bytes INT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE secret_material IS 'Encrypted secret material (ciphertext + envelope metadata)';

--------------------------------------------------------------------------------
-- secret_versions
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS secret_versions (
    version_id TEXT PRIMARY KEY,
    bundle_id TEXT NOT NULL,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    env_id TEXT NOT NULL,
    data_hash TEXT NOT NULL,
    format TEXT NOT NULL,
    material_id TEXT NOT NULL REFERENCES secret_material(material_id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by_actor_id TEXT NOT NULL,
    created_by_actor_type TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_secret_versions_env_id
    ON secret_versions (env_id);

CREATE INDEX IF NOT EXISTS idx_secret_versions_bundle_id
    ON secret_versions (bundle_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_secret_versions_bundle_hash
    ON secret_versions (bundle_id, data_hash);

COMMENT ON TABLE secret_versions IS 'Secret bundle versions (metadata only, no plaintext)';
