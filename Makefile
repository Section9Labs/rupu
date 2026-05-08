.PHONY: build release sign-dev sign-release run install sync gh-build bump fmt lint test gates tui-smoke clean help

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

# Build the release binary locally, then publish it to the rolling
# `latest-build` GitHub release as a `gh release upload --clobber`
# asset. Produces a stable URL — `https://github.com/Section9Labs/rupu/releases/tag/latest-build`
# — that anyone can curl. The tag floats, so do NOT link it from
# CHANGELOG or anywhere that needs a stable version anchor; use the
# `v0.x.y-cli` tags for that.
#
# `release` first because the binary needs to be signed before it
# leaves the laptop. The release itself is unsigned-by-Apple in the
# notarization sense — only `notarize-release.sh` does that — but
# the local Developer ID signature is enough for `xattr -d
# com.apple.quarantine` users.
gh-build: release
	@scripts/gh-build.sh

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
	@echo "  gh-build       release + publish to \`latest-build\` (rolling) AND \`v<X.Y.Z>-build\`"
	@echo "  bump           bump workspace version + commit (usage: make bump VERSION=X.Y.Z)"
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
