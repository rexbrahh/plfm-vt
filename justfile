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
    scripts/dev/with-macos-libiconv.sh cargo build --release -p ghostctl

# Build service images
build-services: build-control-plane build-node-agent build-ingress
    @echo "All service images built."

# Build control-plane service
build-control-plane:
    @echo "Building control-plane..."
    scripts/dev/with-macos-libiconv.sh cargo build --release -p plfm-control-plane

# Build node-agent service
build-node-agent:
    @echo "Building node-agent..."
    scripts/dev/with-macos-libiconv.sh cargo build --release -p plfm-node-agent

# Build ingress service
build-ingress:
    @echo "Building ingress..."
    scripts/dev/with-macos-libiconv.sh cargo build --release -p plfm-ingress

# Build OCI images for all services
build-images: build-services

# Build frontend assets (console + web terminal)
build-web:
    @echo "Building frontend..."
    @if [ -f "frontend/package.json" ]; then \
        (cd frontend && npm run build); \
    else \
        echo "No frontend package.json found; skipping."; \
    fi

# =============================================================================
# Format, Lint, Test
# =============================================================================

# Format all code
fmt:
    @echo "Formatting code..."
    cargo fmt --all
    @if [ -f "go.mod" ]; then gofmt -w .; else echo "No go.mod; skipping gofmt."; fi
    @if [ -f "frontend/package.json" ]; then \
        npx prettier --write 'frontend/**/*.{ts,tsx,js,json,css}'; \
    else \
        echo "No frontend package.json; skipping prettier."; \
    fi

# Check formatting without modifying
fmt-check:
    @echo "Checking formatting..."
    cargo fmt --all --check
    @if [ -f "go.mod" ]; then \
        test -z "$$(gofmt -l .)"; \
    else \
        echo "No go.mod; skipping gofmt check."; \
    fi
    @if [ -f "frontend/package.json" ]; then \
        npx prettier --check 'frontend/**/*.{ts,tsx,js,json,css}'; \
    else \
        echo "No frontend package.json; skipping prettier check."; \
    fi

# Run all linters
lint:
    @echo "Running linters..."
    scripts/dev/with-macos-libiconv.sh cargo clippy --workspace --all-targets --all-features -- -D warnings
    @if [ -f "go.mod" ]; then staticcheck ./...; else echo "No go.mod; skipping staticcheck."; fi
    @if [ -f "frontend/package.json" ]; then (cd frontend && npm run lint); else echo "No frontend package.json; skipping eslint."; fi

# Run all tests
test: test-unit test-integration
    @echo "All tests passed."

# Validate OpenAPI and JSON schemas under api/
api-validate:
    @echo "Validating API schemas..."
    scripts/dev/with-macos-libiconv.sh cargo run -q -p plfm-api-validate

# Run unit tests only
test-unit:
    @echo "Running unit tests..."
    scripts/dev/with-macos-libiconv.sh cargo test --workspace --lib
    @if [ -f "go.mod" ]; then go test ./... -short; else echo "No go.mod; skipping go test."; fi
    @if [ -f "frontend/package.json" ]; then (cd frontend && npm test); else echo "No frontend package.json; skipping frontend tests."; fi

# Run integration tests
test-integration:
    @echo "Running integration tests..."
    scripts/dev/with-macos-libiconv.sh cargo test --workspace --tests
    @if [ -f "go.mod" ]; then go test ./test/integration/...; else echo "No go.mod; skipping go integration tests."; fi

# Run end-to-end tests (requires dev stack)
test-e2e:
    @echo "Running e2e tests..."
    @echo "[placeholder] go test ./test/e2e/..."

# Run performance tests
test-perf:
    @echo "Running performance tests..."
    @echo "[placeholder] go test -bench=. ./test/perf/..."

# Full verification (fmt + lint + test)
verify: fmt-check api-validate lint test
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
    DATABASE_URL=postgres://plfm:plfm_dev@localhost:5432/plfm GHOST_DEV=1 RUST_LOG=debug,sqlx=warn scripts/dev/with-macos-libiconv.sh cargo run -p plfm-control-plane

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
    @if command -v redocly >/dev/null 2>&1; then \
        redocly lint api/openapi/openapi.yaml --skip-rule=no-empty-servers; \
    else \
        echo "redocly not installed; run: npm i -g @redocly/cli"; \
        exit 1; \
    fi

# Validate JSON schemas
validate-schemas:
    @echo "Validating JSON schemas..."
    @if command -v ajv >/dev/null 2>&1; then \
        for schema in api/schemas/*.json; do \
            echo "Validating $schema"; \
            ajv compile -s "$schema" --spec=draft2020 -c ajv-formats; \
        done; \
    else \
        echo "ajv not installed; run: npm i -g ajv-cli ajv-formats"; \
        exit 1; \
    fi

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
