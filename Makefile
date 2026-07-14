.PHONY: build release sign-dev sign-release run install sync gh-build gh-beta gh-stable bump fmt lint test gates app-smoke app-run cp cp-web clean help

# Default target: a quick development build that's already code-signed
# so the macOS keychain doesn't re-prompt on every iteration.
build:
	cargo build -p rupu-cli
	@scripts/sign-dev.sh debug

release:
	cargo build --release -p rupu-cli
	@scripts/sign-dev.sh release

# Build the control-plane web UI, then the CLI that embeds it.
# rupu-cp embeds crates/rupu-cp/web/dist/ at compile time via rust-embed,
# so the web build must run BEFORE the cargo build to embed the real UI
# (otherwise build.rs writes an honest "not built" placeholder).
cp-web:
	cd crates/rupu-cp/web && npm ci && npm run build

cp: cp-web
	cargo build -p rupu-cli

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

# Build + publish a channel release to GitHub via `scripts/gh-build.sh`.
# Each target compiles `target/release/rupu` itself (rather than
# depending on the plain `release` target) so RUPU_RELEASE_CHANNEL /
# RUPU_RELEASE_VERSION are set in the environment FOR THE CARGO BUILD
# STEP — that's what `option_env!` in crates/rupu-cli/src/build_info.rs
# captures at compile time, so the resulting binary's `--version`
# reports its own channel ("beta"/"stable") instead of "dev". The sign
# step is the same `scripts/sign-dev.sh release` the plain `release`
# target uses — the binary needs a local Developer ID signature before
# it leaves the laptop (notarization is a separate, later step).
#
# Publishes both a rolling tag (`latest-beta` / `latest-stable`,
# force-moved every run) and a versioned tag (`v<X.Y.Z>-beta` /
# `v<X.Y.Z>`) — see scripts/gh-build.sh's header for the full channel
# semantics. `rupu update` resolves `[update].channel` against these
# same two channels.
gh-beta:
	RUPU_RELEASE_CHANNEL=beta RUPU_RELEASE_VERSION="$(shell grep -E '^version = "[0-9]+\.[0-9]+\.[0-9]+' Cargo.toml | head -n1 | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+[^"]*)".*/\1/')-beta" cargo build --release -p rupu-cli
	@scripts/sign-dev.sh release
	@scripts/gh-build.sh beta

gh-stable:
	RUPU_RELEASE_CHANNEL=stable RUPU_RELEASE_VERSION="$(shell grep -E '^version = "[0-9]+\.[0-9]+\.[0-9]+' Cargo.toml | head -n1 | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+[^"]*)".*/\1/')" cargo build --release -p rupu-cli
	@scripts/sign-dev.sh release
	@scripts/gh-build.sh stable

# Deprecated: betas used to be published to a rolling `latest-build` /
# `v<X.Y.Z>-build` tag pair. That convention is retired in favor of the
# explicit `beta`/`stable` channel names — this alias exists only so
# muscle-memory `make gh-build` still does something reasonable.
gh-build: gh-beta

# Bump the workspace `[workspace.package].version` in Cargo.toml,
# refresh Cargo.lock to match, and create a `release: bump workspace
# to vX.Y.Z` commit ready for review. Doesn't push — you push when
# the bump is part of a PR you're opening separately, or fold it into
# whatever feature branch you're shipping.
#
# Usage:
#     make bump VERSION=0.5.4
#
# Validation: VERSION must look like X.Y.Z (with optional `-rc.N` etc).
# We refuse to overwrite the same version (no-op detection) so a typo
# doesn't silently produce an empty commit.
bump:
	@if [ -z "$(VERSION)" ]; then \
		echo "usage: make bump VERSION=<X.Y.Z>"; exit 1; \
	fi
	@case "$(VERSION)" in \
		[0-9]*.[0-9]*.[0-9]*) ;; \
		*) echo "VERSION must look like X.Y.Z (got: $(VERSION))"; exit 1 ;; \
	esac
	@CURRENT=$$(grep -E '^version = "[0-9]+\.[0-9]+\.[0-9]+' Cargo.toml | head -n1 | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+[^"]*)".*/\1/'); \
	if [ "$$CURRENT" = "$(VERSION)" ]; then \
		echo "Cargo.toml is already at $(VERSION) — no-op"; exit 0; \
	fi; \
	echo "→ bumping workspace $$CURRENT → $(VERSION)..."; \
	sed -i.bak -E 's/^(version = ")[0-9]+\.[0-9]+\.[0-9]+[^"]*"/\1$(VERSION)"/' Cargo.toml; \
	rm -f Cargo.toml.bak; \
	cargo update -w >/dev/null; \
	git add Cargo.toml Cargo.lock; \
	git commit -m "release: bump workspace to v$(VERSION)" >/dev/null; \
	echo "→ committed: release: bump workspace to v$(VERSION)"; \
	echo "   next:  make gh-build   (or push the branch + PR if you want CI to see the bump first)"

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

# rupu.app — headless smoke test. Builds the binary, launches it
# against the bundled fixture workspace, waits 4 seconds for the
# window to render, then SIGTERMs. Asserts no panic on stderr.
app-smoke:
	@cargo build --release -p rupu-app
	@echo "  · launcher_state test"
	@cargo test -p rupu-app --test launcher_state
	@echo "  · clone helper test"
	@cargo test -p rupu-scm --test clone
	@FIXTURE="$$(pwd)/crates/rupu-app/tests/fixtures/sample-workspace"; \
	OUTPUT=$$(sh -c './target/release/rupu-app "'"$$FIXTURE"'" & sleep 4; kill $$! 2>/dev/null' 2>&1 || true); \
	if echo "$$OUTPUT" | grep -qE 'panic|panicked'; then \
		echo "app-smoke FAIL — panic in output:"; \
		echo "$$OUTPUT"; \
		exit 1; \
	fi; \
	if ! echo "$$OUTPUT" | grep -q 'opened workspace'; then \
		echo "app-smoke FAIL — expected 'opened workspace' log line missing:"; \
		echo "$$OUTPUT"; \
		exit 1; \
	fi
	@echo "app-smoke OK"

app-run:
	@cargo build --release -p rupu-app
	@echo "Running rupu-app against the rupu repo; stderr → /tmp/rupu-app-run.log"
	@sh -c 'RUST_LOG=info,rupu_app=debug,gpui=warn ./target/release/rupu-app . & PID=$$!; sleep 8; kill $$PID 2>/dev/null || true; wait $$PID 2>/dev/null' 2>/tmp/rupu-app-run.log; true
	@if grep -q 'RefCell already borrowed' /tmp/rupu-app-run.log; then \
		echo "FAIL: RefCell errors detected in /tmp/rupu-app-run.log"; \
		grep -c 'RefCell already borrowed' /tmp/rupu-app-run.log; \
		exit 1; \
	fi
	@echo "app-run OK — no RefCell errors during 8s smoke"

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
	@echo "  gh-beta        build (channel=beta) + sign + publish \`latest-beta\` AND \`v<X.Y.Z>-beta\` (prerelease)"
	@echo "  gh-stable      build (channel=stable) + sign + publish \`latest-stable\` AND \`v<X.Y.Z>\` (full release)"
	@echo "  gh-build       deprecated alias for gh-beta (betas used to be tagged \`-build\`)"
	@echo "  bump           bump workspace version + commit (usage: make bump VERSION=X.Y.Z)"
	@echo "  fmt            cargo fmt --all -- --check"
	@echo "  lint           cargo clippy --workspace --all-targets -D warnings"
	@echo "  test           cargo test --workspace"
	@echo "  gates          fmt + lint + test (same as the release-ready check)"
	@echo "  app-smoke      headless 4-second app smoke against bundled fixture"
	@echo "  clean          cargo clean"
	@echo ""
	@echo "Refresh-my-install flow:  make sync && make install"
	@echo ""
	@echo "Override the signing identity with RUPU_SIGNING_IDENTITY=<sha1-or-cn>"
	@echo "On non-macOS hosts the signing step no-ops cleanly."
