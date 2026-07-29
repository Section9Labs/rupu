# Linux Packaging Phase C: Community Formats — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** rupu is installable via `yay -S rupu-bin`, `brew install section9labs/tap/rupu`, and `nix run github:Section9Labs/rupu`.

**Architecture:** All three formats consume the release assets and `.sha256` sidecars the release workflow already publishes — none of them build rupu from source, and none require changes to how releases are produced. Each package definition lives in this repo as the source of truth; a release-time job refreshes their version and hashes so they cannot drift.

**Tech Stack:** AUR `PKGBUILD`, Homebrew formula (Ruby), Nix flake.

**Spec:** `docs/superpowers/specs/2026-07-29-rupu-linux-packaging-design.md` §5

**Depends on:** the release workflow publishing `rupu-{darwin-arm64,linux-x64,linux-arm64}` plus `.sha256` sidecars (Phase A/earlier, merged).

## Two things require maintainer credentials

These block *publishing*, not authoring. Every task below can be built and tested without them:

1. **AUR** needs an account at aur.archlinux.org with an SSH key registered, and the package name reserved. The `PKGBUILD` and its CI check work without it; only `git push` to the AUR remote is blocked.
2. **Homebrew tap** needs a `Section9Labs/homebrew-tap` repository to exist. The formula and its test work in-repo; only publishing to the tap is blocked.

The Nix flake needs nothing external — it lives here.

## Global Constraints

- **None of these build from source.** They download the published binary and verify its checksum. rupu's build needs a pinned musl container and ~16 minutes; asking Arch or Homebrew users to reproduce that is worse for everyone.
- **Checksums must be verified, never skipped.** A formula or PKGBUILD that skips verification is a supply-chain hole with a friendly face.
- **Version and hashes are generated, not hand-edited.** Hand-maintained hashes go stale silently and users get a checksum mismatch they cannot diagnose.
- `linux-x64` → Arch `x86_64`, `linux-arm64` → Arch `aarch64`. Homebrew: `darwin-arm64` → `:arm64_sequoia`-style bottles are NOT used; this is a binary formula, so it selects on `OS.mac?`/`Hardware::CPU`.
- The static musl binary runs on Arch and NixOS unmodified — no glibc concerns.
- Package name is `rupu` everywhere except AUR, where a prebuilt-binary package conventionally takes the `-bin` suffix: **`rupu-bin`**.
- Every change goes through a feature branch and a PR.

---

### Task 1: Nix flake

Nix first because it needs no external account, so it proves the whole "consume published assets by hash" shape before the two blocked formats.

**Files:**
- Create: `flake.nix`
- Create: `flake.lock` (generated)

**Interfaces:**
- Produces: `nix run github:Section9Labs/rupu` and `nix profile install github:Section9Labs/rupu` working on `x86_64-linux`, `aarch64-linux`, `aarch64-darwin`.

- [ ] **Step 1: Write the flake**

```nix
{
  description = "rupu — agentic code-development CLI";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachSystem [
      "x86_64-linux" "aarch64-linux" "aarch64-darwin"
    ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        version = "REPLACED_BY_CI";
        # Asset name per rupu's own convention — the same string
        # `rupu update --print-platform` prints on that platform.
        asset = {
          "x86_64-linux"   = "rupu-linux-x64";
          "aarch64-linux"  = "rupu-linux-arm64";
          "aarch64-darwin" = "rupu-darwin-arm64";
        }.${system};
        sha256 = {
          "x86_64-linux"   = "REPLACED_BY_CI_LINUX_X64";
          "aarch64-linux"  = "REPLACED_BY_CI_LINUX_ARM64";
          "aarch64-darwin" = "REPLACED_BY_CI_DARWIN_ARM64";
        }.${system};
      in {
        packages.default = pkgs.stdenv.mkDerivation {
          pname = "rupu";
          inherit version;
          src = pkgs.fetchurl {
            url = "https://github.com/Section9Labs/rupu/releases/download/v${version}/${asset}";
            inherit sha256;
          };
          dontUnpack = true;
          # The Linux binaries are static musl — no interpreter to patch.
          # Setting this stops nixpkgs' fixup phase from trying.
          dontPatchELF = true;
          dontStrip = true;
          # rupu shells out to ripgrep (hard requirement) and git.
          nativeBuildInputs = [ pkgs.makeWrapper ];
          installPhase = ''
            mkdir -p $out/bin
            install -m755 $src $out/bin/rupu
            wrapProgram $out/bin/rupu \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.ripgrep pkgs.git ]}
          '';
          meta = with pkgs.lib; {
            description = "Agentic code-development CLI";
            homepage = "https://github.com/Section9Labs/rupu";
            license = licenses.asl20;
            platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
            mainProgram = "rupu";
          };
        };
      });
}
```

`wrapProgram` with ripgrep on PATH is the Nix equivalent of the `.deb`'s `Depends: ripgrep` — without it a Nix user gets the same `` `rg` not found `` failure the Linux CI run originally hit.

- [ ] **Step 2: Verify it evaluates**

Nix may not be installed locally. If `command -v nix` fails, say so and rely on Task 4's CI check. If it is available:

```bash
nix flake check --no-build 2>&1 | tail -20
```
Expected: no evaluation errors. The `REPLACED_BY_CI` placeholders make a real build fail — that is expected until Task 4 fills them.

- [ ] **Step 3: Commit**

```bash
git add flake.nix
git commit -m "feat(nix): flake consuming published release binaries"
```

---

### Task 2: AUR PKGBUILD

**Files:**
- Create: `packaging/aur/PKGBUILD`
- Create: `packaging/aur/.SRCINFO` (generated from the PKGBUILD)

**Interfaces:**
- Produces: an AUR-publishable `rupu-bin` package.

- [ ] **Step 1: Write the PKGBUILD**

```bash
# Maintainer: Section9Labs <matias@section9labs.com>
pkgname=rupu-bin
pkgver=REPLACED_BY_CI
pkgrel=1
pkgdesc="Agentic code-development CLI (prebuilt binary)"
arch=('x86_64' 'aarch64')
url="https://github.com/Section9Labs/rupu"
license=('Apache-2.0')
# rupu shells out to rg (hard requirement); git degrades gracefully but is
# expected in practice. ast-grep is optional.
depends=('ripgrep')
optdepends=('git: repository operations'
            'ast-grep: structural search tool')
provides=('rupu')
conflicts=('rupu')
options=('!strip')

source_x86_64=("rupu-$pkgver-x86_64::$url/releases/download/v$pkgver/rupu-linux-x64")
source_aarch64=("rupu-$pkgver-aarch64::$url/releases/download/v$pkgver/rupu-linux-arm64")
sha256sums_x86_64=('REPLACED_BY_CI_LINUX_X64')
sha256sums_aarch64=('REPLACED_BY_CI_LINUX_ARM64')

package() {
  local _bin
  case "$CARCH" in
    x86_64)  _bin="rupu-$pkgver-x86_64" ;;
    aarch64) _bin="rupu-$pkgver-aarch64" ;;
  esac
  install -Dm755 "$srcdir/$_bin" "$pkgdir/usr/bin/rupu"

  # Completions and the man page, generated by the binary itself so they
  # cannot describe a CLI surface it does not have.
  "$pkgdir/usr/bin/rupu" completions print bash --static \
    | install -Dm644 /dev/stdin "$pkgdir/usr/share/bash-completion/completions/rupu"
  "$pkgdir/usr/bin/rupu" completions print zsh --static \
    | install -Dm644 /dev/stdin "$pkgdir/usr/share/zsh/site-functions/_rupu"
  "$pkgdir/usr/bin/rupu" completions print fish --static \
    | install -Dm644 /dev/stdin "$pkgdir/usr/share/fish/vendor_completions.d/rupu.fish"
  "$pkgdir/usr/bin/rupu" man | gzip -9 -n \
    | install -Dm644 /dev/stdin "$pkgdir/usr/share/man/man1/rupu.1.gz"
}
```

`options=('!strip')` matters: the binary is already stripped-ish and static; letting makepkg strip it risks breaking it for no gain.

- [ ] **Step 2: Verify the PKGBUILD parses as shell**

`makepkg` is Arch-only and almost certainly unavailable here. `bash -n` still catches syntax errors:

```bash
bash -n packaging/aur/PKGBUILD && echo "shell syntax ok"
```

Confirm by reading that every `REPLACED_BY_CI` token appears exactly where Task 4's generator expects it.

- [ ] **Step 3: Commit**

```bash
git add packaging/aur/PKGBUILD
git commit -m "feat(aur): PKGBUILD for the rupu-bin package"
```

---

### Task 3: Homebrew formula

**Files:**
- Create: `packaging/homebrew/rupu.rb`

**Interfaces:**
- Produces: a formula publishable to `Section9Labs/homebrew-tap`, installable as `brew install section9labs/tap/rupu`.

- [ ] **Step 1: Write the formula**

```ruby
# Homebrew formula for rupu. Consumes the published release binaries —
# building from source would need a pinned musl container and ~16 minutes.
class Rupu < Formula
  desc "Agentic code-development CLI"
  homepage "https://github.com/Section9Labs/rupu"
  version "REPLACED_BY_CI"
  license "Apache-2.0"

  # rupu shells out to `rg` and hard-errors without it. This is the
  # equivalent of the .deb's `Depends: ripgrep`.
  depends_on "ripgrep"

  on_macos do
    on_arm do
      url "https://github.com/Section9Labs/rupu/releases/download/v#{version}/rupu-darwin-arm64"
      sha256 "REPLACED_BY_CI_DARWIN_ARM64"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/Section9Labs/rupu/releases/download/v#{version}/rupu-linux-x64"
      sha256 "REPLACED_BY_CI_LINUX_X64"
    end
    on_arm do
      url "https://github.com/Section9Labs/rupu/releases/download/v#{version}/rupu-linux-arm64"
      sha256 "REPLACED_BY_CI_LINUX_ARM64"
    end
  end

  def install
    bin.install Dir["*"].first => "rupu"
    generate_completions_from_executable(bin/"rupu", "completions", "print",
                                         shells: [:bash, :zsh, :fish],
                                         shell_parameter_format: :none)
    man1.install Utils.safe_popen_read(bin/"rupu", "man") => "rupu.1"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/rupu --version")
    assert_match "linux-x64", shell_output("#{bin}/rupu update --print-platform") if OS.linux? && Hardware::CPU.intel?
  end
end
```

If `generate_completions_from_executable`'s parameter shape does not fit rupu's `completions print <shell> --static` surface, fall back to writing the three files explicitly with `Utils.safe_popen_read`. Verify which works rather than assuming — state which you used.

- [ ] **Step 2: Verify the Ruby parses**

```bash
ruby -c packaging/homebrew/rupu.rb
```
Expected: `Syntax OK`. (`brew audit` needs a tap and Homebrew's Ruby; skip it here and note that.)

- [ ] **Step 3: Commit**

```bash
git add packaging/homebrew/rupu.rb
git commit -m "feat(homebrew): formula consuming published release binaries"
```

---

### Task 4: Keep them in sync — the `community` job

Hand-maintained versions and hashes go stale silently. This job rewrites all three definitions from the release that just happened.

**Files:**
- Create: `packaging/sync-community.sh`
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `meta.outputs.version`, and the `.sha256` sidecars from the `asset-*` artifacts.
- Produces: updated `flake.nix`, `packaging/aur/PKGBUILD`, `packaging/homebrew/rupu.rb`, committed back to `main`.

- [ ] **Step 1: Write the sync script**

```bash
#!/usr/bin/env bash
# packaging/sync-community.sh <version> <sha-dir>
#
# Rewrites the version and checksums in the three community package
# definitions from the sidecars the release just published. Hand-editing
# these is how they go stale, and a stale hash surfaces to the user as a
# checksum mismatch they cannot diagnose.
set -euo pipefail

VERSION="${1:?usage: sync-community.sh <version> <sha-dir>}"
SHADIR="${2:?usage: sync-community.sh <version> <sha-dir>}"

sha_for() {
  local asset="$1" f="$SHADIR/$1.sha256"
  test -f "$f" || { echo "missing sidecar: $f" >&2; exit 1; }
  # Sidecar format is "<hex>  <asset-name>"
  awk '{print $1}' "$f"
}

X64=$(sha_for rupu-linux-x64)
ARM64=$(sha_for rupu-linux-arm64)
DARWIN=$(sha_for rupu-darwin-arm64)

for v in "$X64" "$ARM64" "$DARWIN"; do
  [ "${#v}" -eq 64 ] || { echo "not a sha256: $v" >&2; exit 1; }
done

for f in flake.nix packaging/aur/PKGBUILD packaging/homebrew/rupu.rb; do
  test -f "$f" || { echo "missing: $f" >&2; exit 1; }
  sed -i.bak \
    -e "s/REPLACED_BY_CI_LINUX_X64/$X64/g" \
    -e "s/REPLACED_BY_CI_LINUX_ARM64/$ARM64/g" \
    -e "s/REPLACED_BY_CI_DARWIN_ARM64/$DARWIN/g" \
    -e "s/REPLACED_BY_CI/$VERSION/g" \
    "$f"
  rm -f "$f.bak"
done

# Nothing may still carry a placeholder — a leftover means a rename broke
# the substitution and the definition would ship unusable.
if grep -rn "REPLACED_BY_CI" flake.nix packaging/aur/PKGBUILD packaging/homebrew/rupu.rb; then
  echo "placeholders remain after substitution" >&2
  exit 1
fi

echo "synced community definitions to $VERSION"
```

Note the substitution order: the three suffixed tokens are replaced *before* the bare `REPLACED_BY_CI`, because the bare token is a prefix of all of them. Reversing the order would corrupt every hash. The trailing grep is what catches that class of mistake.

- [ ] **Step 2: Verify**

```bash
chmod +x packaging/sync-community.sh
bash -n packaging/sync-community.sh && echo "shell syntax ok"
```

Then dry-run it against fabricated sidecars in a temp dir and confirm all three files come out with no placeholders and correct-length hashes. Restore the files afterwards (`git checkout --`).

- [ ] **Step 3: Add the `community` job**

```yaml
  community:
    name: sync community package definitions
    needs: [meta, publish]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v5
        with:
          ref: main
      - uses: actions/download-artifact@v4
        with:
          pattern: asset-linux-*
          merge-multiple: true
          path: shas
      - uses: actions/download-artifact@v4
        with:
          name: asset-darwin-arm64
          path: shas
      - name: Sync definitions
        run: ./packaging/sync-community.sh "${{ needs.meta.outputs.version }}" shas
      - name: Commit if changed
        run: |
          set -euo pipefail
          if git diff --quiet; then
            echo "definitions already current"; exit 0
          fi
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
          git add flake.nix packaging/aur/PKGBUILD packaging/homebrew/rupu.rb
          git commit -m "chore(packaging): sync community definitions to ${{ needs.meta.outputs.version }}"
          git push origin HEAD:main
```

This job commits to `main`. That is deliberate and narrow — it touches exactly three generated files — but it must never run on a beta, or `main` would carry beta versions in its published definitions. Gate it: `if: needs.meta.outputs.channel == 'stable'`.

- [ ] **Step 4: Verify YAML + shell, then commit**

---

### Task 5: Publish — the parts needing maintainer credentials

**Files:**
- Modify: `docs/RELEASING.md`
- Modify: `README.md`

- [ ] **Step 1: Document the AUR publish path**

The AUR is a git remote. Publishing is:

```bash
git clone ssh://aur@aur.archlinux.org/rupu-bin.git aur-rupu-bin
cp packaging/aur/PKGBUILD aur-rupu-bin/
cd aur-rupu-bin && makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO && git commit -m "Update to <version>" && git push
```

Document that this needs an AUR account with an SSH key registered, and that `.SRCINFO` must be regenerated on every version bump or the AUR rejects the push. Do NOT automate this in CI until the account exists — an SSH deploy key for AUR is a separate secret with its own rotation story.

- [ ] **Step 2: Document the Homebrew tap path**

The tap is a plain GitHub repo named `homebrew-tap` under the org, with formulae in `Formula/`. Publishing is copying `packaging/homebrew/rupu.rb` to `Formula/rupu.rb` there and pushing. Document that the repo must be created first, and that `brew install section9labs/tap/rupu` then works with no further setup for users.

- [ ] **Step 3: README — add the three install paths**

Under the existing Install section, add Arch (`yay -S rupu-bin`), Homebrew (`brew install section9labs/tap/rupu`), and Nix (`nix run github:Section9Labs/rupu`). Mark AUR and Homebrew as pending publication if the account and tap do not exist yet — do not advertise an install path that 404s.

- [ ] **Step 4: Commit**

---

## Self-Review

**Spec coverage (§5).** AUR PKGBUILD → Task 2. Homebrew tap formula → Task 3. Nix flake → Task 1. All three consume published release assets and checksums rather than building from source → enforced by each definition's `url`/`source` pointing at the release download URL. Keeping them from drifting → Task 4, which the spec did not require but without which they rot within two releases.

**Placeholder scan.** The literal string `REPLACED_BY_CI` appears deliberately in three files and is substituted by Task 4's script, which then greps for leftovers and fails if any survive. No "TBD"/"TODO".

**Type consistency.** The placeholder tokens (`REPLACED_BY_CI`, `REPLACED_BY_CI_LINUX_X64`, `REPLACED_BY_CI_LINUX_ARM64`, `REPLACED_BY_CI_DARWIN_ARM64`) are spelled identically in Tasks 1, 2, 3 and in Task 4's `sed`. Asset names (`rupu-linux-x64`, `rupu-linux-arm64`, `rupu-darwin-arm64`) match what the release workflow publishes and what `rupu update --print-platform` reports.

**Known risks.**
- Task 4's `sed` substitution order is load-bearing: the bare token is a prefix of the three suffixed ones, so it must be replaced last. The trailing placeholder grep is the guard.
- The `community` job pushes to `main`. It is gated to stable releases and touches three generated files, but it is still a bot writing to the default branch — worth a branch-protection exception rather than disabling protection.
- Homebrew's `generate_completions_from_executable` expects a particular argument shape; rupu's is `completions print <shell> --static`. Task 3 says to verify rather than assume, with an explicit fallback.
- AUR and Homebrew publication are blocked on maintainer-created accounts. Task 5 documents them rather than automating them, deliberately: an AUR SSH deploy key is a secret with its own rotation story and should not be added speculatively.
