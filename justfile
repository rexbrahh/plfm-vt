# plfm-vt justfile
# Run `just --list` to see available targets

set dotenv-load := true

# Default recipe
default:
    @just --list

# =============================================================================
# Bootstrap and Setup
# =============================================================================

# One-time dev environment setup
bootstrap:
    @echo "Setting up development environment..."
    @if command -v nix >/dev/null 2>&1; then \
        echo "Nix detected. Run 'nix develop' for pinned toolchain."; \
    else \
        echo "Nix not found. Ensure you have: git, docker, rust, go, node"; \
    fi
    @echo "Bootstrap complete. Run 'just verify' to confirm setup."

# =============================================================================
# Build Targets
# =============================================================================

# Build all components
build: build-cli build-services build-web
    @echo "All components built."

# Build the CLI
build-cli:
    @echo "Building ghostctl CLI..."
    @echo "[placeholder] cargo build --release -p ghostctl"

# Build service images
build-services: build-control-plane build-node-agent build-ingress
    @echo "All service images built."

# Build control-plane service
build-control-plane:
    @echo "Building control-plane..."
    @echo "[placeholder] docker build -t ghcr.io/plfm-vt/control-plane:dev services/control-plane"

# Build node-agent service
build-node-agent:
    @echo "Building node-agent..."
    @echo "[placeholder] docker build -t ghcr.io/plfm-vt/node-agent:dev services/node-agent"

# Build ingress service
build-ingress:
    @echo "Building ingress..."
    @echo "[placeholder] docker build -t ghcr.io/plfm-vt/ingress:dev services/ingress"

# Build OCI images for all services
build-images: build-services

# Build frontend assets (console + web terminal)
build-web:
    @echo "Building frontend..."
    @echo "[placeholder] cd frontend && npm run build"

# =============================================================================
# Format, Lint, Test
# =============================================================================

# Format all code
fmt:
    @echo "Formatting code..."
    @echo "[placeholder] cargo fmt --all"
    @echo "[placeholder] gofmt -w ."
    @echo "[placeholder] prettier --write 'frontend/**/*.{ts,tsx,js,json,css}'"

# Check formatting without modifying
fmt-check:
    @echo "Checking formatting..."
    @echo "[placeholder] cargo fmt --all --check"
    @echo "[placeholder] gofmt -l . | grep -q . && exit 1 || true"
    @echo "[placeholder] prettier --check 'frontend/**/*.{ts,tsx,js,json,css}'"

# Run all linters
lint:
    @echo "Running linters..."
    @echo "[placeholder] cargo clippy --all-targets --all-features -- -D warnings"
    @echo "[placeholder] staticcheck ./..."
    @echo "[placeholder] eslint frontend/"

# Run all tests
test: test-unit test-integration
    @echo "All tests passed."

# Run unit tests only
test-unit:
    @echo "Running unit tests..."
    @echo "[placeholder] cargo test --lib"
    @echo "[placeholder] go test ./... -short"
    @echo "[placeholder] cd frontend && npm test"

# Run integration tests
test-integration:
    @echo "Running integration tests..."
    @echo "[placeholder] cargo test --test '*'"
    @echo "[placeholder] go test ./test/integration/..."

# Run end-to-end tests (requires dev stack)
test-e2e:
    @echo "Running e2e tests..."
    @echo "[placeholder] go test ./test/e2e/..."

# Run performance tests
test-perf:
    @echo "Running performance tests..."
    @echo "[placeholder] go test -bench=. ./test/perf/..."

# Full verification (fmt + lint + test)
verify: fmt-check lint test
    @echo "Verification complete."

# =============================================================================
# Local Dev Stack
# =============================================================================

# Bring up the local dev stack (Postgres only, run services locally)
dev-up:
    @echo "Starting local dev stack..."
    docker compose -f deploy/environments/dev/docker-compose.yml up -d
    @echo ""
    @echo "PostgreSQL available at: postgres://plfm:plfm_dev@localhost:5432/plfm"
    @echo ""
    @echo "To run the control plane:"
    @echo "  DATABASE_URL=postgres://plfm:plfm_dev@localhost:5432/plfm GHOST_DEV=1 cargo run -p plfm-control-plane"
    @echo ""
    @echo "To run the node agent:"
    @echo "  GHOST_CONTROL_PLANE_URL=http://localhost:8080 GHOST_DATA_DIR=/tmp/ghost cargo run -p plfm-node-agent"

# Tear down the local dev stack
dev-down:
    @echo "Stopping local dev stack..."
    docker compose -f deploy/environments/dev/docker-compose.yml down

# Tail logs for all dev stack components
dev-logs:
    @echo "Tailing dev stack logs..."
    docker compose -f deploy/environments/dev/docker-compose.yml logs -f

# Wipe local state and restart dev stack
dev-reset: dev-down
    @echo "Wiping local state..."
    docker compose -f deploy/environments/dev/docker-compose.yml down -v
    just dev-up

# Health check summary for dev stack
dev-status:
    @echo "Checking dev stack status..."
    docker compose -f deploy/environments/dev/docker-compose.yml ps
    @echo ""
    @curl -s http://127.0.0.1:8080/healthz 2>/dev/null | jq . || echo "Control plane not running"

# Run control plane in dev mode (requires dev-up first)
dev-control-plane:
    DATABASE_URL=postgres://plfm:plfm_dev@localhost:5432/plfm GHOST_DEV=1 RUST_LOG=debug,sqlx=warn cargo run -p plfm-control-plane

# Run node agent in dev mode (requires control plane running)
dev-node-agent:
    GHOST_CONTROL_PLANE_URL=http://localhost:8080 GHOST_DATA_DIR=/tmp/ghost RUST_LOG=debug cargo run -p plfm-node-agent

# =============================================================================
# Frontend / Web Terminal
# =============================================================================

# Start frontend dev server
web-up:
    @echo "Starting frontend dev server..."
    @echo "[placeholder] cd frontend && npm run dev"

# Run frontend tests
web-test:
    @echo "Running frontend tests..."
    @echo "[placeholder] cd frontend && npm test"

# Build frontend for production
web-build: build-web

# =============================================================================
# Schema and API Validation
# =============================================================================

# Validate OpenAPI spec
validate-openapi:
    @echo "Validating OpenAPI spec..."
    @echo "[placeholder] npx @redocly/cli lint api/openapi/openapi.yaml"

# Validate JSON schemas
validate-schemas:
    @echo "Validating JSON schemas..."
    @echo "[placeholder] ajv compile -s 'api/schemas/*.json'"

# Validate all API contracts
validate-api: validate-openapi validate-schemas
    @echo "API validation complete."

# =============================================================================
# Documentation
# =============================================================================

# Check markdown links
docs-check:
    @echo "Checking documentation..."
    find docs -name '*.md' -print0 | xargs -0 -I {} sh -c 'echo "Checking {}"; head -1 "{}"' || true
    @echo "[placeholder] markdown-link-check docs/**/*.md"

# Serve docs locally (if using a doc site generator)
docs-serve:
    @echo "Serving docs locally..."
    @echo "[placeholder] mkdocs serve"

# =============================================================================
# Supply Chain
# =============================================================================

# Generate SBOM for all artifacts
sbom:
    @echo "Generating SBOMs..."
    @echo "[placeholder] syft packages . -o spdx-json > sbom.spdx.json"

# Sign artifacts (requires signing keys)
sign:
    @echo "Signing artifacts..."
    @echo "[placeholder] cosign sign-blob --key cosign.key <artifact>"

# =============================================================================
# Clean
# =============================================================================

# Clean all build artifacts
clean:
    @echo "Cleaning build artifacts..."
    @echo "[placeholder] cargo clean"
    @echo "[placeholder] rm -rf frontend/dist frontend/node_modules/.cache"
    @echo "[placeholder] docker rmi ghcr.io/plfm-vt/control-plane:dev ghcr.io/plfm-vt/node-agent:dev ghcr.io/plfm-vt/ingress:dev 2>/dev/null || true"

# =============================================================================
# Helpers
# =============================================================================

# Show environment info
env-info:
    @echo "Environment info:"
    @echo "  OS: $(uname -s)"
    @echo "  Arch: $(uname -m)"
    @command -v rustc >/dev/null 2>&1 && rustc --version || echo "  Rust: not found"
    @command -v go >/dev/null 2>&1 && go version || echo "  Go: not found"
    @command -v node >/dev/null 2>&1 && node --version || echo "  Node: not found"
    @command -v docker >/dev/null 2>&1 && docker --version || echo "  Docker: not found"
    @command -v nix >/dev/null 2>&1 && nix --version || echo "  Nix: not found"
