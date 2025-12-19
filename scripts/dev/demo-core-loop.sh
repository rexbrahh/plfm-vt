#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Demo: control-plane core loop (request-id + idempotency + RYW)

Usage:
  scripts/dev/demo-core-loop.sh [--reset] [--keep-running]

Options:
  --reset          Wipe dev DB volume (docker compose down -v) before starting.
  --keep-running   Leave control-plane running after the demo finishes.
EOF
}

RESET=0
KEEP_RUNNING=0

while [ $# -gt 0 ]; do
    case "$1" in
    --reset) RESET=1 ;;
    --keep-running) KEEP_RUNNING=1 ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        echo "Unknown argument: $1" >&2
        usage
        exit 2
        ;;
    esac
    shift
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

require_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Missing required command: $1" >&2
        exit 1
    fi
}

require_cmd docker
require_cmd curl
require_cmd jq
require_cmd just

DATABASE_URL="${DATABASE_URL:-postgres://plfm:plfm_dev@localhost:5432/plfm}"
API_URL="${VT_API_URL:-http://localhost:8080}"
API_V1="${API_URL%/}/v1"

echo "Dev stack: ${DATABASE_URL}"
echo "API base:  ${API_URL}"

if [ "${RESET}" -eq 1 ]; then
    echo "Resetting dev stack..."
    just dev-reset
else
    just dev-up
fi

TMP_DIR="$(mktemp -d)"
CP_LOG="${TMP_DIR}/control-plane.log"

echo "Starting control-plane (logs: ${CP_LOG})..."
DATABASE_URL="${DATABASE_URL}" \
    GHOST_DEV=1 \
    GHOST_LOG_LEVEL=info \
    RUST_LOG=info,sqlx=warn \
    scripts/dev/with-macos-libiconv.sh cargo run -p plfm-control-plane >"${CP_LOG}" 2>&1 &
CP_PID=$!

cleanup() {
    if [ "${KEEP_RUNNING}" -eq 1 ]; then
        echo "Leaving control-plane running (pid=${CP_PID})."
        echo "Logs: ${CP_LOG}"
        return 0
    fi

    if kill -0 "${CP_PID}" >/dev/null 2>&1; then
        kill "${CP_PID}" >/dev/null 2>&1 || true
    fi
}

trap cleanup EXIT

echo "Waiting for /healthz..."
for _ in $(seq 1 120); do
    if curl -fsS "${API_URL%/}/healthz" >/dev/null 2>&1; then
        break
    fi
    sleep 0.25
done

if ! curl -fsS "${API_URL%/}/healthz" >/dev/null 2>&1; then
    echo "Control-plane did not become healthy. Tail log:" >&2
    tail -n 50 "${CP_LOG}" >&2 || true
    exit 1
fi

echo "Healthy."

api_post_json() {
    local path="$1"
    local idempotency_key="$2"
    local json_body="$3"
    local out_prefix="$4"

    local headers="${TMP_DIR}/${out_prefix}.headers"
    local body="${TMP_DIR}/${out_prefix}.json"

    local status
    status="$(
        curl -sS \
            -X POST \
            -H "Content-Type: application/json" \
            -H "Idempotency-Key: ${idempotency_key}" \
            -D "${headers}" \
            -o "${body}" \
            -w "%{http_code}" \
            "${API_V1}${path}" \
            --data "${json_body}"
    )"

    if [ "${status}" -lt 200 ] || [ "${status}" -ge 300 ]; then
        echo "POST ${path} failed (${status})" >&2
        cat "${body}" >&2 || true
        exit 1
    fi

    local request_id
    request_id="$(
        grep -i '^x-request-id:' "${headers}" | head -n1 | awk '{print $2}' | tr -d '\r'
    )"
    if [ -z "${request_id}" ]; then
        echo "Missing X-Request-Id header for POST ${path}" >&2
        cat "${headers}" >&2 || true
        exit 1
    fi

    echo "${request_id}"
}

assert_equal() {
    local label="$1"
    local a="$2"
    local b="$3"
    if [ "${a}" != "${b}" ]; then
        echo "Assertion failed (${label}):" >&2
        echo "  expected: ${a}" >&2
        echo "  got:      ${b}" >&2
        exit 1
    fi
}

now_suffix="$(date +%s)"
org_name="demo-org-${now_suffix}"
app_name="demo-app-${now_suffix}"
env_name="prod"

echo ""
echo "== Create org (with idempotency replay) =="
org_body="$(jq -c -n --arg name "${org_name}" '{name: $name}')"
idem_org="demo-org-${now_suffix}-key"
req1="$(api_post_json "/orgs" "${idem_org}" "${org_body}" "org-1")"
org_id="$(jq -r '.id' "${TMP_DIR}/org-1.json")"
echo "request_id=${req1} org_id=${org_id}"

req2="$(api_post_json "/orgs" "${idem_org}" "${org_body}" "org-2")"
org_id_2="$(jq -r '.id' "${TMP_DIR}/org-2.json")"
assert_equal "org idempotency" "${org_id}" "${org_id_2}"
echo "replay_request_id=${req2} org_id=${org_id_2} (same)"

echo ""
echo "== Create app =="
app_body="$(jq -c -n --arg name "${app_name}" --arg desc "demo app" '{name: $name, description: $desc}')"
idem_app="demo-app-${now_suffix}-key"
req_app="$(api_post_json "/orgs/${org_id}/apps" "${idem_app}" "${app_body}" "app")"
app_id="$(jq -r '.id' "${TMP_DIR}/app.json")"
echo "request_id=${req_app} app_id=${app_id}"

echo ""
echo "== Create env =="
env_body="$(jq -c -n --arg name "${env_name}" '{name: $name}')"
idem_env="demo-env-${now_suffix}-key"
req_env="$(api_post_json "/apps/${app_id}/envs" "${idem_env}" "${env_body}" "env")"
env_id="$(jq -r '.id' "${TMP_DIR}/env.json")"
echo "request_id=${req_env} env_id=${env_id}"

echo ""
echo "== Create release (with idempotency replay) =="
image_digest="sha256:0000000000000000000000000000000000000000000000000000000000000000"
manifest_hash="sha256:1111111111111111111111111111111111111111111111111111111111111111"
release_body="$(
    jq -c -n \
        --arg image_ref "example.com/${app_name}:demo" \
        --arg image_digest "${image_digest}" \
        --arg manifest_hash "${manifest_hash}" \
        '{image_ref: $image_ref, image_digest: $image_digest, manifest_schema_version: 1, manifest_hash: $manifest_hash}'
)"
idem_release="demo-release-${now_suffix}-key"
req_rel_1="$(
    api_post_json "/orgs/${org_id}/apps/${app_id}/releases" "${idem_release}" "${release_body}" "release-1"
)"
release_id="$(jq -r '.id' "${TMP_DIR}/release-1.json")"
echo "request_id=${req_rel_1} release_id=${release_id}"

req_rel_2="$(
    api_post_json "/orgs/${org_id}/apps/${app_id}/releases" "${idem_release}" "${release_body}" "release-2"
)"
release_id_2="$(jq -r '.id' "${TMP_DIR}/release-2.json")"
assert_equal "release idempotency" "${release_id}" "${release_id_2}"
echo "replay_request_id=${req_rel_2} release_id=${release_id_2} (same)"

echo ""
echo "== Create deploy =="
deploy_body="$(jq -c -n --arg release_id "${release_id}" '{release_id: $release_id, process_types: null, is_rollback: false}')"
idem_deploy="demo-deploy-${now_suffix}-key"
req_dep="$(api_post_json "/orgs/${org_id}/apps/${app_id}/envs/${env_id}/deploys" "${idem_deploy}" "${deploy_body}" "deploy")"
deploy_id="$(jq -r '.id' "${TMP_DIR}/deploy.json")"
echo "request_id=${req_dep} deploy_id=${deploy_id}"

echo ""
echo "== Create route (with idempotency replay) =="
route_body="$(
    jq -c -n \
        --arg hostname "example-${now_suffix}.invalid" \
        '{hostname: $hostname, listen_port: 443, protocol_hint: "tls_passthrough", backend_process_type: "web", backend_port: 8080, proxy_protocol: "off", backend_expects_proxy_protocol: false, ipv4_required: false}'
)"
idem_route="demo-route-${now_suffix}-key"
req_route_1="$(
    api_post_json "/orgs/${org_id}/apps/${app_id}/envs/${env_id}/routes" "${idem_route}" "${route_body}" "route-1"
)"
route_id="$(jq -r '.id' "${TMP_DIR}/route-1.json")"
echo "request_id=${req_route_1} route_id=${route_id}"

req_route_2="$(
    api_post_json "/orgs/${org_id}/apps/${app_id}/envs/${env_id}/routes" "${idem_route}" "${route_body}" "route-2"
)"
route_id_2="$(jq -r '.id' "${TMP_DIR}/route-2.json")"
assert_equal "route idempotency" "${route_id}" "${route_id_2}"
echo "replay_request_id=${req_route_2} route_id=${route_id_2} (same)"

echo ""
echo "== Projection checkpoints (debug) =="
curl -fsS "${API_V1}/_debug/projections" | jq .

echo ""
echo "Demo complete."
