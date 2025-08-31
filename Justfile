set dotenv-load := false

# --- Rust workflows ---

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

lint: fmt-check clippy

test:
	OLLAMA_MAX_ELAPSED_SECS=1 cargo nextest run --all-features

watch:
	cargo watch -x "nextest run --all-features"

audit:
	cargo audit

deny:
	cargo deny check

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
cov:
        cargo llvm-cov clean --workspace
        cargo llvm-cov --all-features --workspace --lcov --output-path target/lcov.info

startup:
  @echo "Startup complete"

teardown: ollama-stop grip-stop
  @echo "Teardown complete"
