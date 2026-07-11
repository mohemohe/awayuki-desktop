# Maintainer: mohemohe <mohemohe@ghippos.net>

pkgname=awayuki
# Derived from `git describe` so this file never needs a version bump. The
# upstream release scheme is the `v*` tag itself (see .github/workflows/
# release.yml — the workflow extracts VERSION from the git tag, and Cargo.toml
# `version` is unused at release time).
#
#   - HEAD on `vX.Y.Z` exactly  -> X.Y.Z
#   - HEAD past `vX.Y.Z`        -> X.Y.Z.r<N>.g<hash>   (Arch VCS convention)
#   - Source archive (no .git)  -> Cargo.toml version
#   - Untagged git checkout     -> r<commit-count>.g<hash>
pkgver=$(
    cd "${startdir:-$PWD}" 2>/dev/null
    if _tag=$(git describe --tags --exact-match --match 'v*' HEAD 2>/dev/null); then
        # pkgver disallows hyphens; pre-release tags like v0.5.0-rc1 -> 0.5.0.rc1.
        printf '%s' "${_tag#v}" | tr '-' '.'
    elif _desc=$(git describe --tags --long --abbrev=7 --match 'v*' HEAD 2>/dev/null); then
        printf '%s' "${_desc#v}" | sed 's/\([^-]*-g\)/r\1/;s/-/./g'
    elif [ ! -d .git ] && [ -f Cargo.toml ]; then
        sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1
    else
        _count=$(git rev-list --count HEAD 2>/dev/null || echo 0)
        _hash=$(git rev-parse --short=7 HEAD 2>/dev/null || echo 0000000)
        printf 'r%s.g%s' "$_count" "$_hash"
    fi
)
pkgrel=1
pkgdesc="A Tauri desktop client for Mastodon, Misskey, Paon, and Bluesky"
arch=('x86_64' 'aarch64')
url="https://github.com/mohemohe/awayuki-desktop"
license=('custom:WTFPL')
depends=(
    'gtk3'
    'webkit2gtk-4.1'
    'libayatana-appindicator'
    'librsvg'
    'openssl'
    'sqlite'
    'glib2'
    'libxkbcommon'
    'gcc-libs'
    'glibc'
)
makedepends=(
    'bun'
    'rust'
    'cargo'
    'git'
    'pkgconf'
    'clang'
    'cmake'
)
optdepends=(
    'libnotify: desktop notifications'
)
provides=("$pkgname")
conflicts=("$pkgname")
options=('!lto' '!debug')

# `makepkg -si` builds an exact checkout or an extracted release source archive.
# Release CI creates the archive from the verified source commit and records its
# SHA-256 before invoking makepkg in a clean Arch container.
source=()
sha256sums=()

prepare() {
    cd "$startdir"
    bun install --frozen-lockfile
    cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
    cd "$startdir"
    # Keep makepkg build artifacts out of $startdir/target (used by the
    # developer's `cargo run`) and out of $srcdir (which is $startdir/src,
    # the Rust source directory — a makepkg/Rust naming collision specific
    # to this repo). Stage them under build/ alongside other release outputs.
    export CARGO_TARGET_DIR="$startdir/build/arch-target"
    # build.rs wires VERSION into APP_VERSION; mirror what release.yml does.
    export VERSION="$pkgver"
    bun run build
    cargo build --locked --release
}

package() {
    install -Dm755 "$startdir/build/arch-target/release/awayuki" \
        "$pkgdir/usr/bin/awayuki"

    install -Dm644 "$startdir/LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"

    install -Dm644 "$startdir/assets/icons/AppIcon.png" \
        "$pkgdir/usr/share/icons/hicolor/512x512/apps/awayuki.png"

    install -Dm644 "$startdir/packaging/awayuki.desktop" \
        "$pkgdir/usr/share/applications/awayuki.desktop"
}
