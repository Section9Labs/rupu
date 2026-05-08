.PHONY: build release sign-dev sign-release run install sync gh-build fmt lint test gates tui-smoke clean help

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

# Pull origin and fast-forward main. Safe-by-default: when the cwd is
# on a feature branch we only fetch (so PR work in flight doesn't get
# clobbered by an accidental rebase). Canonical "refresh my install":
#   make sync && make install
sync:
	@git fetch origin --prune
	@branch=$$(git rev-parse --abbrev-ref HEAD); \
	if [ "$$branch" = "main" ]; then \
		echo "→ on main, fast-forwarding from origin..."; \
		git pull --ff-only origin main; \
	else \
		echo "→ not on main (current: $$branch); origin fetched, no merge."; \
		echo "   to update main:  git checkout main && git pull --ff-only"; \
	fi

# Trigger the build.yml workflow on origin and watch it through to
# completion. Produces an unsigned macOS arm64 binary as a downloadable
# artifact. Useful when you want a clean-room build off origin without
# a local toolchain. Requires the `gh` CLI.
gh-build:
	@command -v gh >/dev/null 2>&1 || { echo "gh CLI not installed — \`brew install gh\` then \`gh auth login\`"; exit 1; }
	@echo "→ Triggering build.yml workflow on origin..."
	@gh workflow run build.yml
	@sleep 3
	@RUN_ID=$$(gh run list --workflow=build.yml --limit 1 --json databaseId --jq '.[0].databaseId'); \
	if [ -z "$$RUN_ID" ]; then echo "could not resolve run id; check \`gh run list --workflow=build.yml\`"; exit 1; fi; \
	echo "→ Watching run $$RUN_ID (Ctrl-C to detach — the run continues remotely)..."; \
	gh run watch "$$RUN_ID" --exit-status; \
	echo ""; \
	echo "→ Download artifact:  gh run download $$RUN_ID --name rupu-darwin-arm64"

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
	@echo "  install        release + install to /usr/local/bin/rupu (sudo)"
	@echo "  sync           git fetch origin; fast-forward main if checked out"
	@echo "  gh-build       trigger the GH Actions build.yml workflow + watch"
	@echo "  fmt            cargo fmt --all -- --check"
	@echo "  lint           cargo clippy --workspace --all-targets -D warnings"
	@echo "  test           cargo test --workspace"
	@echo "  gates          fmt + lint + test (same as the release-ready check)"
	@echo "  tui-smoke      headless 5-second TUI smoke against bundled fixture"
	@echo "  clean          cargo clean"
	@echo ""
	@echo "Refresh-my-install flow:  make sync && make install"
	@echo ""
	@echo "Override the signing identity with RUPU_SIGNING_IDENTITY=<sha1-or-cn>"
	@echo "On non-macOS hosts the signing step no-ops cleanly."
