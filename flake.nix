{
  description = "Zcash Forum ETL: Rust toolchain + static analysis + tests + Postgres";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # ---- Single source of truth for local PG ----
        pg = {
          name = "zc_forum";
          port = "54329";
          user = "postgres";
          host = "127.0.0.1";
          data = ".nix/pgdata";
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "clippy" "rustfmt" "rust-src" ];
        };

        devPkgs = with pkgs; [
          rustToolchain
          cargo-nextest
          cargo-deny
          cargo-audit
          cargo-watch
          sqlx-cli
          just
          postgresql_15
          pkg-config
          openssl
          protobuf
          jq curl git
          nixpkgs-fmt
          cargo-binstall
          ollama
          python313Packages.grip
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          packages = devPkgs;

          # Exported env (apps/CLI use DATABASE_URL; PG* is local sugar)
          PGHOST = pg.host;
          PGPORT = pg.port;
          PGUSER = pg.user;
          PGDATABASE = pg.name;
          PGPASSWORD = "";
          PGDATA = pg.data;
          DATABASE_URL = "postgresql://${pg.user}@${pg.host}:${pg.port}/${pg.name}";
          LLM_SUMMARIZER = "ollama";
          LLM_MODEL = "qwen2.5:latest";
          OLLAMA_BASE_URL = "http://127.0.0.1:11434";

          shellHook = ''
            mkdir -p .nix

            # Ensure cargo-llvm-cov exists (once)
            if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
              echo "Installing cargo-llvm-cov via cargo-binstall..."
              cargo binstall -y cargo-llvm-cov@0.6.11 || cargo binstall -y cargo-llvm-cov
            fi

            # prepend marker to your shell prompt
            export PS1="\[\033[1;36m\](zc-forum)\[\033[0m\] $PS1"

            # start "daemons"
            just startup

            # stop Postgres when shell exits
            trap 'echo "Tearing down"; just teardown || true' EXIT

            echo "Rust: $(rustc --version)"
            echo "Postgres: $PGHOST:$PGPORT db=$PGDATABASE (data: $PGDATA)"
            echo
            echo "Helpers:"
            echo "  just pg-start     # start local Postgres"
            echo "  just pg-stop      # stop local Postgres"
            echo "  just db-create    # create '${pg.name}'"
            echo "  just test         # nextest"
            echo "  just lint         # clippy + fmt check"
            echo "  just cov          # coverage (llvm-cov)"
            echo
            echo "Doc server available at http://localhost:6419 (grip)"
            echo
          '';
        };

        devShells.ci = pkgs.mkShell {
          packages = devPkgs;
          PGHOST = pg.host;
          PGPORT = pg.port;
          PGUSER = pg.user;
          PGDATABASE = pg.name;
          PGDATA = pg.data;
          DATABASE_URL = "postgresql://${pg.user}@${pg.host}:${pg.port}/${pg.name}";
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}

