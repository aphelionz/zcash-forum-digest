set dotenv-load := false

# --- Postgres lifecycle (uses env from flake shell) ---

pg-init:
	@if [ ! -d "$PGDATA" ]; then \
		echo "Initializing Postgres at $PGDATA..."; \
		initdb -D "$PGDATA" -A trust -U "${PGUSER:-postgres}" >/dev/null; \
		echo "port = ${PGPORT:-54329}" >> "$PGDATA/postgresql.conf"; \
		echo "listen_addresses = '127.0.0.1,::1'" >> "$PGDATA/postgresql.conf"; \
		echo "host all all 127.0.0.1/32 trust" >> "$PGDATA/pg_hba.conf"; \
		echo "host all all ::1/128      trust" >> "$PGDATA/pg_hba.conf"; \
	fi

pg-start: pg-init
	pg_ctl -D "$PGDATA" -l .nix/pg.log start
	@echo "Postgres started on ${PGHOST:-127.0.0.1}:${PGPORT:-54329} (log: .nix/pg.log)"

pg-stop:
	@if [ -d "$PGDATA" ]; then pg_ctl -D "$PGDATA" stop; fi
	@echo "Postgres stopped"

pg-status:
	pg_ctl -D "$PGDATA" status || true

pg-reset: pg-stop
	rm -rf "$PGDATA"
	@echo "Reset complete"

db-create:
	createdb -h "${PGHOST:-127.0.0.1}" -p "${PGPORT:-54329}" "${PGDATABASE:-zc_forum}" || echo "DB may already exist"

# --- Rust workflows ---

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

lint: fmt-check clippy

test:
	cargo nextest run --all-features

watch:
	cargo watch -x "nextest run --all-features"

audit:
	cargo audit

deny:
	cargo deny check

cov:
	cargo llvm-cov clean --workspace
	cargo llvm-cov --all-features --workspace --lcov --output-path target/lcov.info

# --- SQLx helpers ---

sqlx-prepare:
	SQLX_OFFLINE=true cargo sqlx prepare

migrate:
	sqlx migrate run

migrate-add MIGRATION_NAME:
	sqlx migrate add {{MIGRATION_NAME}}

# Wipe PG data dir, start fresh, and (re)create the DB
db-reset:
	@echo "Stopping Postgres (if running)…"
	-just pg-stop
	@echo "Removing data dir: $PGDATA"
	rm -rf "$PGDATA"
	@echo "Starting fresh Postgres…"
	just pg-start
	@echo "Creating database: ${PGDATABASE:-zc_forum}"
	just db-create
	@echo "Done."

# --- Ollama lifecycle ---
ollama-start:
  ollama serve &

ollama-stop:
  @pkill -f "ollama serve" || true

# --- Documentation tasks ---
grip-start:
  @grip --quiet . &

grip-stop:
  @pkill -f "grip" || true

# --- Composite tasks ---
startup: pg-start ollama-start db-create grip-start
  @echo "Startup complete"

teardown: pg-stop ollama-stop grip-stop
  @echo "Teardown complete"
