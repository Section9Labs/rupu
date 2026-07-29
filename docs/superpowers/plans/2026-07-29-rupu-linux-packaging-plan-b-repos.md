# Linux Packaging Phase B: Hosted APT / YUM Repositories — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `sudo apt install rupu` and `sudo dnf install rupu` work after a two-line setup, and `apt upgrade` delivers new versions.

**Architecture:** A `repo` job runs after `publish`, downloads the four packages the release just produced, signs them and the repository indices with a dedicated GPG key, and pushes an APT + YUM tree to GitHub Pages. The index carries only the current version; every version's packages remain permanently downloadable as GitHub release assets.

**Tech Stack:** GitHub Actions, GitHub Pages, `dpkg-scanpackages` / `apt-ftparchive`, `createrepo_c`, `gpg`, `rpmsign`.

**Spec:** `docs/superpowers/specs/2026-07-29-rupu-linux-packaging-design.md` §3

**Depends on:** Phase A (PR #562, merged) — the `packages` job and its `asset-packages` artifact.

**Already done before this plan starts:** the signing key exists (RSA 4096, sign-only, fingerprint `6A2918F205000696D657AC61D672DF3BE13ADFDD`, expires 2031-07-28). Its private half is stored outside the repo by the maintainer, and CI secrets `GPG_PRIVATE_KEY` (ASCII-armored private key) and `GPG_KEY_ID` (`D672DF3BE13ADFDD`) are registered.

## Global Constraints

- **The index carries only the current version.** GitHub Pages soft-caps near 1 GB; each version is ~100 MiB across two architectures and two formats. Every version stays permanently installable from its GitHub release assets. Do not accumulate versions in the index.
- **Never write the private key to the repository, to `dist/`, or to any path under the Pages publish root.** It exists only as a CI secret, decrypted into `$RUNNER_TEMP`, and removed before the job ends.
- **The public key must be served from a stable URL** that never changes across releases: `https://section9labs.github.io/rupu/rupu-archive-keyring.asc`. Users pin trust to it; moving it breaks every existing installation.
- **Signing must actually be verified, not assumed.** A repo that publishes with a broken signature fails *on the user's machine*, at install time, with a confusing error. CI must verify before publishing.
- Beta and stable must not collide. They are separate suites in one repository, so a user opting into beta never silently receives stable, or vice versa.
- Package name `rupu` never changes.
- Every change goes through a feature branch and a PR.

---

### Task 1: Publish the public key and the repository skeleton

Users must be able to fetch the signing key before any package exists, and the setup snippet must be stable from day one.

**Files:**
- Create: `.github/workflows/pages.yml` OR extend the release workflow — decide by reading how `release.yml` is structured, and prefer extending it if the publish step can simply push to the `gh-pages` branch.
- Create: `docs/pages/index.html` (a minimal landing page naming the repo URLs)
- Create: `docs/pages/rupu-archive-keyring.asc` (the ASCII-armored PUBLIC key)

**Interfaces:**
- Produces: `https://section9labs.github.io/rupu/rupu-archive-keyring.asc` serving the public key.

- [ ] **Step 1: Add the public key**

The public key is safe to commit — it is public by definition. Export it from the maintainer's keyring or take it from the CI secret's counterpart:

```bash
gpg --armor --export D672DF3BE13ADFDD > docs/pages/rupu-archive-keyring.asc
```

Verify before committing:

```bash
gpg --show-keys docs/pages/rupu-archive-keyring.asc
```
Expected: fingerprint `6A2918F205000696D657AC61D672DF3BE13ADFDD`, capability `[SC]`, expiry 2031-07-28, and **no secret key material** — the file must contain `BEGIN PGP PUBLIC KEY BLOCK` and must NOT contain `PRIVATE`.

- [ ] **Step 2: Assert the file is a public key, in CI**

Add a check to `ci.yml` so a private key can never be committed here by accident:

```yaml
      - name: Assert the published keyring is public-only
        run: |
          set -euo pipefail
          f=docs/pages/rupu-archive-keyring.asc
          grep -q "BEGIN PGP PUBLIC KEY BLOCK" "$f" || { echo "::error::$f is not a public key block"; exit 1; }
          if grep -q "PRIVATE KEY BLOCK" "$f"; then
            echo "::error::$f contains private key material"; exit 1
          fi
```

- [ ] **Step 3: Enable GitHub Pages**

Pages must serve from a branch or from Actions. Check the current setting:

```bash
gh api repos/Section9Labs/rupu/pages 2>/dev/null || echo "Pages not enabled"
```

If not enabled, enable it with the `gh-pages` branch as source (or GitHub Actions as source, if that suits the publish job better — decide and state which, do not leave it ambiguous).

- [ ] **Step 4: Commit**

```bash
git add docs/pages .github/workflows/ci.yml
git commit -m "ci(pages): publish the package signing public key"
```

---

### Task 2: Build and sign the APT repository

**Files:**
- Create: `packaging/build-apt-repo.sh`

**Interfaces:**
- Consumes: a directory of `.deb` files, the channel name (`beta`|`stable`), and an imported signing key.
- Produces: a tree at `<out>/apt/` containing `dists/<channel>/…/Packages{,.gz}`, `dists/<channel>/Release`, `Release.gpg`, `InRelease`, and `pool/main/`.

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# packaging/build-apt-repo.sh <deb-dir> <channel> <out-dir>
#
# Builds a signed flat-ish APT repository. Beta and stable are separate
# SUITES in one repository, so a user who opted into beta never silently
# receives stable.
#
# Only the CURRENT version is indexed; older versions stay downloadable
# from their GitHub release assets. See the spec's "Why the index carries
# only the latest version".
set -euo pipefail

DEB_DIR="${1:?usage: build-apt-repo.sh <deb-dir> <channel> <out-dir>}"
CHANNEL="${2:?usage: build-apt-repo.sh <deb-dir> <channel> <out-dir>}"
OUT="${3:?usage: build-apt-repo.sh <deb-dir> <channel> <out-dir>}"

case "$CHANNEL" in
  beta|stable) ;;
  *) echo "channel must be beta or stable (got: $CHANNEL)" >&2; exit 1 ;;
esac

POOL="$OUT/apt/pool/main"
DIST="$OUT/apt/dists/$CHANNEL/main"
mkdir -p "$POOL" "$DIST/binary-amd64" "$DIST/binary-arm64"

cp "$DEB_DIR"/*.deb "$POOL/"

for arch in amd64 arm64; do
  ( cd "$OUT/apt" && dpkg-scanpackages --arch "$arch" pool/ /dev/null ) \
    > "$DIST/binary-$arch/Packages"
  gzip -9 -k -f "$DIST/binary-$arch/Packages"
  # A Packages file with no entries means the pool did not contain a .deb
  # for this architecture — publishing that silently gives users an
  # "unable to locate package" they cannot diagnose.
  test -s "$DIST/binary-$arch/Packages" \
    || { echo "no .deb found for $arch" >&2; exit 1; }
done

apt-ftparchive \
  -o "APT::FTPArchive::Release::Origin=Section9Labs" \
  -o "APT::FTPArchive::Release::Label=rupu" \
  -o "APT::FTPArchive::Release::Suite=$CHANNEL" \
  -o "APT::FTPArchive::Release::Codename=$CHANNEL" \
  -o "APT::FTPArchive::Release::Architectures=amd64 arm64" \
  -o "APT::FTPArchive::Release::Components=main" \
  release "$OUT/apt/dists/$CHANNEL" > "$OUT/apt/dists/$CHANNEL/Release"

gpg --batch --yes --armor --detach-sign \
  -o "$OUT/apt/dists/$CHANNEL/Release.gpg" "$OUT/apt/dists/$CHANNEL/Release"
gpg --batch --yes --clearsign \
  -o "$OUT/apt/dists/$CHANNEL/InRelease" "$OUT/apt/dists/$CHANNEL/Release"

echo "APT repo built for channel $CHANNEL:"
find "$OUT/apt" -type f | sort
```

- [ ] **Step 2: Make executable and syntax-check**

```bash
chmod +x packaging/build-apt-repo.sh
bash -n packaging/build-apt-repo.sh && echo "shell syntax ok"
```

- [ ] **Step 3: Commit**

```bash
git add packaging/build-apt-repo.sh
git commit -m "build(packaging): signed APT repository builder"
```

---

### Task 3: Build and sign the YUM repository

**Files:**
- Create: `packaging/build-yum-repo.sh`

**Interfaces:**
- Consumes: a directory of `.rpm` files, the channel, an imported signing key, and `GPG_KEY_ID`.
- Produces: `<out>/yum/<channel>/` containing the rpms and a signed `repodata/repomd.xml`.

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# packaging/build-yum-repo.sh <rpm-dir> <channel> <out-dir>
#
# RPM signing differs from deb: the PACKAGES themselves are signed with
# rpmsign (deb packages are not individually signed; the Release file
# covers them), and repomd.xml gets a detached signature.
set -euo pipefail

RPM_DIR="${1:?usage: build-yum-repo.sh <rpm-dir> <channel> <out-dir>}"
CHANNEL="${2:?usage: build-yum-repo.sh <rpm-dir> <channel> <out-dir>}"
OUT="${3:?usage: build-yum-repo.sh <rpm-dir> <channel> <out-dir>}"
: "${GPG_KEY_ID:?GPG_KEY_ID must be set}"

case "$CHANNEL" in
  beta|stable) ;;
  *) echo "channel must be beta or stable (got: $CHANNEL)" >&2; exit 1 ;;
esac

REPO="$OUT/yum/$CHANNEL"
mkdir -p "$REPO"
cp "$RPM_DIR"/*.rpm "$REPO/"

# rpmsign needs the key name in ~/.rpmmacros
cat > "$HOME/.rpmmacros" <<RPMMACROS
%_signature gpg
%_gpg_name $GPG_KEY_ID
RPMMACROS

rpmsign --addsign "$REPO"/*.rpm

# Verify the signature took. rpmsign can exit 0 having signed nothing
# useful if the macro is wrong; an unsigned rpm in a gpgcheck=1 repo
# fails on the user's machine, not here.
for f in "$REPO"/*.rpm; do
  rpm -qpi "$f" 2>/dev/null | grep -qiE "Signature *:.*Key ID" \
    || { echo "rpm not signed: $f" >&2; exit 1; }
done

createrepo_c "$REPO"
gpg --batch --yes --armor --detach-sign -o "$REPO/repodata/repomd.xml.asc" \
  "$REPO/repodata/repomd.xml"

echo "YUM repo built for channel $CHANNEL:"
find "$REPO" -type f | sort
```

- [ ] **Step 2: Make executable and syntax-check**

```bash
chmod +x packaging/build-yum-repo.sh
bash -n packaging/build-yum-repo.sh && echo "shell syntax ok"
```

- [ ] **Step 3: Commit**

```bash
git add packaging/build-yum-repo.sh
git commit -m "build(packaging): signed YUM repository builder"
```

---

### Task 4: The `repo` job — build, verify, publish to Pages

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `asset-packages` artifact; `meta.outputs.channel`; secrets `GPG_PRIVATE_KEY`, `GPG_KEY_ID`.
- Produces: the Pages site at `https://section9labs.github.io/rupu/`.

- [ ] **Step 1: Add the job**

```yaml
  repo:
    name: publish apt/yum repositories
    needs: [meta, publish]
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v5

      - name: Install repository tooling
        run: |
          set -euo pipefail
          sudo apt-get update -qq
          sudo apt-get install -y -qq dpkg-dev apt-utils createrepo-c rpm

      - uses: actions/download-artifact@v4
        with:
          name: asset-packages
          path: pkgs

      # The key lives in RUNNER_TEMP, never under the checkout or the
      # Pages publish root — a signing key committed to a public repo is
      # unrecoverable.
      - name: Import the signing key
        env:
          GPG_PRIVATE_KEY: ${{ secrets.GPG_PRIVATE_KEY }}
        run: |
          set -euo pipefail
          export GNUPGHOME="$RUNNER_TEMP/gnupg"
          mkdir -p "$GNUPGHOME" && chmod 700 "$GNUPGHOME"
          echo "GNUPGHOME=$GNUPGHOME" >> "$GITHUB_ENV"
          printf '%s' "$GPG_PRIVATE_KEY" | gpg --batch --import
          gpg --list-secret-keys --keyid-format=long

      - name: Build the signed repositories
        env:
          GPG_KEY_ID: ${{ secrets.GPG_KEY_ID }}
          CHANNEL: ${{ needs.meta.outputs.channel }}
        run: |
          set -euo pipefail
          mkdir -p site
          cp docs/pages/index.html site/ 2>/dev/null || true
          cp docs/pages/rupu-archive-keyring.asc site/
          ./packaging/build-apt-repo.sh pkgs "$CHANNEL" site
          ./packaging/build-yum-repo.sh pkgs "$CHANNEL" site

      # Verify BEFORE publishing. A broken signature fails on the user's
      # machine at install time with an error they cannot diagnose.
      - name: Verify the signatures
        env:
          CHANNEL: ${{ needs.meta.outputs.channel }}
        run: |
          set -euo pipefail
          gpg --verify "site/apt/dists/$CHANNEL/Release.gpg" \
                       "site/apt/dists/$CHANNEL/Release"
          gpg --verify "site/apt/dists/$CHANNEL/InRelease"
          gpg --verify "site/yum/$CHANNEL/repodata/repomd.xml.asc" \
                       "site/yum/$CHANNEL/repodata/repomd.xml"
          echo "signatures verified"

      - name: Confirm no private key material is about to be published
        run: |
          set -euo pipefail
          if grep -rl "PRIVATE KEY BLOCK" site/ 2>/dev/null; then
            echo "::error::private key material found in the publish root"; exit 1
          fi
          echo "publish root is clean"

      - uses: actions/upload-pages-artifact@v3
        with:
          path: site
      - id: deploy
        uses: actions/deploy-pages@v4
```

The job needs `permissions: pages: write` and `id-token: write`. Add them at job level rather than widening the workflow's top-level `permissions`.

- [ ] **Step 2: Verify the YAML parses and every run block is valid shell**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('yaml ok')"
```
Then `bash -n` every `run:` block, as the Phase A tasks did.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): publish signed apt/yum repositories to Pages"
```

---

### Task 5: Verify a real user install path, end to end

Building a repository is not the same as a user being able to install from it. This task proves the actual `apt install rupu` path.

**Files:**
- Modify: `.github/workflows/release.yml` (add verification to the `repo` job)

- [ ] **Step 1: Add a Debian install-from-repo check**

This runs against the *just-built local tree*, not the live Pages site — Pages takes time to propagate, and a release must not depend on that latency.

```yaml
      - name: Verify apt can install from the built repo
        env:
          CHANNEL: ${{ needs.meta.outputs.channel }}
        run: |
          set -euo pipefail
          docker run --rm -v "$PWD/site":/repo debian:12 bash -c '
            set -euo pipefail
            apt-get update -qq
            apt-get install -y -qq ca-certificates gnupg
            cp /repo/rupu-archive-keyring.asc /etc/apt/trusted.gpg.d/rupu.asc
            echo "deb [signed-by=/etc/apt/trusted.gpg.d/rupu.asc] file:///repo/apt '"$CHANNEL"' main" \
              > /etc/apt/sources.list.d/rupu.list
            apt-get update -qq
            apt-get install -y -qq rupu
            rupu --version
            command -v rg >/dev/null || { echo "ripgrep dependency not resolved"; exit 1; }
          '
```

This is the assertion that matters: it proves the signature is trusted, the index resolves, and the dependency is pulled — the three things that break silently.

- [ ] **Step 2: Add a Fedora install-from-repo check**

```yaml
      - name: Verify dnf can install from the built repo
        env:
          CHANNEL: ${{ needs.meta.outputs.channel }}
        run: |
          set -euo pipefail
          docker run --rm -v "$PWD/site":/repo fedora:41 bash -c '
            set -euo pipefail
            rpm --import /repo/rupu-archive-keyring.asc
            cat > /etc/yum.repos.d/rupu.repo <<REPO
          [rupu]
          name=rupu
          baseurl=file:///repo/yum/'"$CHANNEL"'
          enabled=1
          gpgcheck=1
          repo_gpgcheck=1
          gpgkey=file:///repo/rupu-archive-keyring.asc
          REPO
            dnf install -y -q rupu
            rupu --version
            command -v rg >/dev/null || { echo "ripgrep dependency not resolved"; exit 1; }
          '
```

`gpgcheck=1` and `repo_gpgcheck=1` are both deliberate: the first proves the packages are signed, the second proves the repository metadata is. Do not relax either to make the step pass — that is the whole point of Phase B.

- [ ] **Step 3: Verify YAML and shell, then commit**

---

### Task 6: Document the setup, and publish the install snippets

**Files:**
- Modify: `README.md`
- Modify: `docs/pages/index.html`
- Modify: `.github/workflows/release.yml` (release-notes body)

- [ ] **Step 1: README — add repository setup**

Debian/Ubuntu, using the modern deb822 format with the legacy one-liner noted (Ubuntu LTS releases still in support only understand the latter):

```
sudo curl -fsSL -o /etc/apt/keyrings/rupu.asc \
  https://section9labs.github.io/rupu/rupu-archive-keyring.asc
echo "deb [signed-by=/etc/apt/keyrings/rupu.asc] https://section9labs.github.io/rupu/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/rupu.list
sudo apt update && sudo apt install rupu
```

Fedora/RHEL:

```
sudo rpm --import https://section9labs.github.io/rupu/rupu-archive-keyring.asc
sudo tee /etc/yum.repos.d/rupu.repo <<'EOF'
[rupu]
name=rupu
baseurl=https://section9labs.github.io/rupu/yum/stable
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://section9labs.github.io/rupu/rupu-archive-keyring.asc
EOF
sudo dnf install rupu
```

Document the beta channel as the same snippets with `stable` → `beta`, and state plainly that the repository indexes only the current version — pinning an older one means downloading that release's `.deb`/`.rpm` directly.

- [ ] **Step 2: Landing page** — `docs/pages/index.html` names the two repo URLs, the key fingerprint, and links to the README section. Keep it plain HTML; no build step.

- [ ] **Step 3: Release notes** — add a line pointing at the repository setup, so someone landing on a release page finds it.

- [ ] **Step 4: Commit**

---

## Self-Review

**Spec coverage (§3).** Signing key → done before this plan (fingerprint recorded above). Public key at a stable URL → Task 1. APT `Packages`/`Release` + detached signature → Task 2. YUM `repodata` + signed `repomd.xml` → Task 3. Published to Pages → Task 4. Latest-version-only index → Tasks 2 and 3 build from the current release's packages only; nothing accumulates. Two-line user setup documented → Task 6.

**Placeholder scan.** No "TBD"/"TODO". Task 1 Step 3 requires a decision (Pages source: branch vs Actions) and says to state which rather than leave it ambiguous — that is a real fork whose answer depends on the repo's current Pages configuration, which the implementer must read.

**Type consistency.** `build-apt-repo.sh <deb-dir> <channel> <out-dir>` and `build-yum-repo.sh <rpm-dir> <channel> <out-dir>` take the same argument shape and are invoked that way in Task 4. `GPG_KEY_ID` is required by the YUM script and supplied from the secret in Task 4. The channel value flows from `meta.outputs.channel` (`beta`|`stable`) into both scripts and both verification steps, and both scripts reject anything else.

**Known risks.**
- `dpkg-scanpackages` emits paths relative to the directory it runs in; the script `cd`s into `$OUT/apt` for exactly that reason. If the `Filename:` entries come out wrong, apt reports a 404 the user cannot diagnose — Task 5's install check is what catches it.
- `rpmsign` requires a `~/.rpmmacros` naming the key; the script writes one. It can exit 0 without signing if the macro is wrong, which is why Task 3 verifies each rpm afterwards rather than trusting the exit code.
- GitHub Pages serves from a single root; beta and stable share it as separate suites/directories. A user who adds both channels gets both — documented, not prevented.
- Pages propagation is not instant. Task 5 verifies against the local tree deliberately, so a release never fails on CDN latency.
