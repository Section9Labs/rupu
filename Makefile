.PHONY: build release sign-dev sign-release run install sync bump fmt lint test gates cp cp-web clean help macos-gen macos-build macos-test macos-run macos-fixtures

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
	echo "   next:  push the branch + PR so main picks up the bump; releases are cut by CI (gh workflow run release-beta.yml), not from a laptop"

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

# rupu.app (macOS, Swift) — XcodeGen scaffold under apps/rupu-macos/.
# macos-gen regenerates the gitignored .xcodeproj from project.yml;
# macos-build/-run depend on it so the project is always fresh.
macos-gen:
	xcodegen generate --spec apps/rupu-macos/project.yml

macos-build: macos-gen
	xcodebuild -project apps/rupu-macos/rupu.xcodeproj -scheme rupu \
		-configuration Debug -derivedDataPath apps/rupu-macos/DerivedData \
		CODE_SIGNING_ALLOWED=NO build

macos-test:
	swift test --package-path apps/rupu-macos/RupuKit

macos-run: macos-build
	open apps/rupu-macos/DerivedData/Build/Products/Debug/rupu.app

# Regenerate the golden JSON fixtures the Swift app's decode/encode tests
# check against (apps/rupu-macos/Fixtures/*.json,
# apps/rupu-macos/Fixtures/requests/*.json). Run this after changing
# rupu_orchestrator::executor::Event, HostInfoResponse, or any of the
# write-path DTOs/request bodies, then update the Swift models to match.
macos-fixtures:
	REGEN_FIXTURES=1 cargo test -p rupu-cp fixture_is_current
	REGEN_FIXTURES=1 cargo test -p rupu-cp request_fixture_roundtrips

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
	@echo "  bump           bump workspace version + commit (usage: make bump VERSION=X.Y.Z)"
	@echo "  fmt            cargo fmt --all -- --check"
	@echo "  lint           cargo clippy --workspace --all-targets -D warnings"
	@echo "  test           cargo test --workspace"
	@echo "  gates          fmt + lint + test (same as the release-ready check)"
	@echo "  clean          cargo clean"
	@echo ""
	@echo "  macos-gen      xcodegen generate apps/rupu-macos/project.yml"
	@echo "  macos-build    macos-gen + xcodebuild the rupu scheme (Debug, unsigned)"
	@echo "  macos-test     swift test the RupuKit package"
	@echo "  macos-run      macos-build + open the built rupu.app"
	@echo "  macos-fixtures regenerate apps/rupu-macos/Fixtures/*.json golden fixtures"
	@echo ""
	@echo "Refresh-my-install flow:  make sync && make install"
	@echo ""
	@echo "Override the signing identity with RUPU_SIGNING_IDENTITY=<sha1-or-cn>"
	@echo "On non-macOS hosts the signing step no-ops cleanly."
