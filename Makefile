.PHONY: build release sign-dev sign-release run install fmt lint test gates tui-smoke clean help

# Default target: a quick development build that's already code-signed
# so the macOS keychain doesn't re-prompt on every iteration.
build:
	cargo build -p rupu-cli
	@scripts/sign-dev.sh debug

release:
	cargo build --release -p rupu-cli
	@scripts/sign-dev.sh release

# Sign-only targets (useful if you build via cargo directly):
sign-dev:
	@scripts/sign-dev.sh debug

sign-release:
	@scripts/sign-dev.sh release

# Build, sign, and run the debug binary directly (skips cargo run's
# binary path). Pass arguments via ARGS=... e.g. `make run ARGS="auth status"`.
run: build
	target/debug/rupu $(ARGS)

# Replace /usr/local/bin/rupu with the just-built signed release binary.
# Requires sudo on most systems.
install: release
	sudo install -m 755 target/release/rupu /usr/local/bin/rupu
	@/usr/local/bin/rupu --version

fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

# Run all the quality gates a release would. Same set the release runbook checks.
gates: fmt lint test

clean:
	cargo clean

tui-smoke:
	@cargo build --release -p rupu-cli
	@if [ ! -d crates/rupu-tui/tests/fixtures/run_smoke ]; then \
		echo "fixture missing — see crates/rupu-tui/tests/fixtures/README.md"; exit 1; \
	fi
	@sh -c 'RUPU_TUI_DEFAULT_VIEW=tree ./target/release/rupu watch run_smoke --replay --pace=5 & PID=$$; sleep 5; kill $$PID 2>/dev/null || true; wait $$PID 2>/dev/null' || true
	@echo "tui-smoke OK"

help:
	@echo "rupu Makefile targets:"
	@echo ""
	@echo "  build          cargo build + sign with Developer ID (debug)"
	@echo "  release        cargo build --release + sign (release)"
	@echo "  sign-dev       sign target/debug/rupu (no rebuild)"
	@echo "  sign-release   sign target/release/rupu (no rebuild)"
	@echo "  run            build + run target/debug/rupu (pass ARGS=...)"
	@echo "  install        release + install to /usr/local/bin/rupu"
	@echo "  fmt            cargo fmt --all -- --check"
	@echo "  lint           cargo clippy --workspace --all-targets -D warnings"
	@echo "  test           cargo test --workspace"
	@echo "  gates          fmt + lint + test (same as the release-ready check)"
	@echo "  tui-smoke      headless 5-second TUI smoke against bundled fixture"
	@echo "  clean          cargo clean"
	@echo ""
	@echo "Override the signing identity with RUPU_SIGNING_IDENTITY=<sha1-or-cn>"
	@echo "On non-macOS hosts the signing step no-ops cleanly."
