# Maintainer: mohemohe <mohemohe@ghippos.net>

pkgname=awayuki
# Derived from `git describe` so this file never needs a version bump. The
# upstream release scheme is the `v*` tag itself (see .github/workflows/
# release.yml — the workflow extracts VERSION from the git tag, and Cargo.toml
# `version` is unused at release time).
#
#   - HEAD on `vX.Y.Z` exactly  -> X.Y.Z
#   - HEAD past `vX.Y.Z`        -> X.Y.Z.r<N>.g<hash>   (Arch VCS convention)
#   - No matching tag           -> r<commit-count>.g<hash>
pkgver=$(
    cd "${startdir:-$PWD}" 2>/dev/null
    if _tag=$(git describe --tags --exact-match --match 'v*' HEAD 2>/dev/null); then
        # pkgver disallows hyphens; pre-release tags like v0.5.0-rc1 -> 0.5.0.rc1.
        printf '%s' "${_tag#v}" | tr '-' '.'
    elif _desc=$(git describe --tags --long --abbrev=7 --match 'v*' HEAD 2>/dev/null); then
        printf '%s' "${_desc#v}" | sed 's/\([^-]*-g\)/r\1/;s/-/./g'
    else
        _count=$(git rev-list --count HEAD 2>/dev/null || echo 0)
        _hash=$(git rev-parse --short=7 HEAD 2>/dev/null || echo 0000000)
        printf 'r%s.g%s' "$_count" "$_hash"
    fi
)
pkgrel=1
pkgdesc="A lightweight Mastodon / Pleroma / Akkoma client with TweetDeck-like multi-column UI"
arch=('x86_64' 'aarch64')
url="https://github.com/mohemohe/awayuki-desktop"
license=('custom:WTFPL')
depends=(
    'fontconfig'
    'libxkbcommon'
    'libxcb'
    'wayland'
    'vulkan-icd-loader'
    'gcc-libs'
    'glibc'
)
makedepends=(
    'rust'
    'cargo'
    'git'
    'pkgconf'
    'clang'
    'cmake'
)
optdepends=(
    'vulkan-radeon: Vulkan support for AMD GPUs'
    'vulkan-intel: Vulkan support for Intel GPUs'
    'nvidia-utils: Vulkan support for NVIDIA GPUs'
    'libnotify: desktop notifications'
)
provides=("$pkgname")
conflicts=("$pkgname")
options=('!lto' '!debug')

# `makepkg -si` is expected to run inside the cloned repository, so the build
# operates directly on the checkout via $startdir instead of fetching a tarball.
source=()
sha256sums=()

prepare() {
    cd "$startdir"
    export RUSTUP_TOOLCHAIN=stable
    cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
    cd "$startdir"
    export RUSTUP_TOOLCHAIN=stable
    # Keep makepkg build artifacts out of $startdir/target (used by the
    # developer's `cargo run`) and out of $srcdir (which is $startdir/src,
    # the Rust source directory — a makepkg/Rust naming collision specific
    # to this repo). Stage them under build/ alongside other release outputs.
    export CARGO_TARGET_DIR="$startdir/build/arch-target"
    # build.rs wires VERSION into APP_VERSION; mirror what release.yml does.
    export VERSION="$pkgver"
    cargo build --locked --release
}

package() {
    install -Dm755 "$startdir/build/arch-target/release/awayuki" \
        "$pkgdir/usr/bin/awayuki"

    install -Dm644 "$startdir/LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"

    install -Dm644 "$startdir/assets/icons/AppIcon.png" \
        "$pkgdir/usr/share/icons/hicolor/512x512/apps/awayuki.png"

    install -dm755 "$pkgdir/usr/share/applications"
    cat > "$pkgdir/usr/share/applications/awayuki.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Awayuki
GenericName=Mastodon Client
Comment=A lightweight Mastodon / Paon / Misskey / Bluesky client
Exec=awayuki
Icon=awayuki
Categories=Network;
Terminal=false
StartupWMClass=Awayuki
EOF
}
