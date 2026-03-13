#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────
#  p2pfiletransfer — setup script (Linux / macOS / WSL)
# ──────────────────────────────────────────────────────────
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { echo -e "${CYAN}[info]${NC}  $*"; }
ok()    { echo -e "${GREEN}[ok]${NC}    $*"; }
warn()  { echo -e "${YELLOW}[warn]${NC}  $*"; }
err()   { echo -e "${RED}[error]${NC} $*"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo ""
echo "══════════════════════════════════════════"
echo "  p2pfiletransfer setup"
echo "══════════════════════════════════════════"
echo ""

# ── detect OS ─────────────────────────────────────────────

OS="$(uname -s)"
ARCH="$(uname -m)"
info "detected: $OS ($ARCH)"

case "$OS" in
    Linux*)  PLATFORM="linux"  ;;
    Darwin*) PLATFORM="macos"  ;;
    MINGW*|MSYS*|CYGWIN*) PLATFORM="windows" ;;
    *) err "unsupported OS: $OS" ;;
esac

# ── install system dependencies ───────────────────────────

install_deps() {
    info "checking system dependencies..."

    case "$PLATFORM" in
        linux)
            if command -v apt-get &>/dev/null; then
                info "debian/ubuntu detected — installing build-essential, pkg-config, libssl-dev"
                sudo apt-get update -qq
                sudo apt-get install -y -qq build-essential pkg-config libssl-dev curl
            elif command -v dnf &>/dev/null; then
                info "fedora/rhel detected — installing gcc, openssl-devel"
                sudo dnf install -y gcc pkg-config openssl-devel curl
            elif command -v pacman &>/dev/null; then
                info "arch detected — installing base-devel, openssl"
                sudo pacman -Sy --needed --noconfirm base-devel openssl pkg-config curl
            elif command -v zypper &>/dev/null; then
                info "opensuse detected — installing gcc, openssl-devel"
                sudo zypper install -y gcc pkg-config libopenssl-devel curl
            elif command -v apk &>/dev/null; then
                info "alpine detected — installing build-base, openssl-dev"
                sudo apk add build-base pkgconfig openssl-dev curl
            else
                warn "unknown package manager — make sure gcc, pkg-config, openssl headers are installed"
            fi
            ;;
        macos)
            if ! command -v brew &>/dev/null; then
                warn "homebrew not found — install from https://brew.sh if builds fail"
            fi
            # macOS ships with clang, and openssl is usually available via LibreSSL
            if ! xcode-select -p &>/dev/null; then
                info "installing xcode command line tools..."
                xcode-select --install 2>/dev/null || warn "xcode CLI tools prompt opened — rerun after install"
            fi
            ;;
        windows)
            warn "on Windows, ensure Visual Studio Build Tools (C++ workload) are installed"
            warn "download from: https://visualstudio.microsoft.com/visual-cpp-build-tools/"
            ;;
    esac

    ok "system dependencies ready"
}

# ── install rust ──────────────────────────────────────────

install_rust() {
    if command -v rustc &>/dev/null; then
        local ver
        ver="$(rustc --version)"
        ok "rust already installed: $ver"
        return
    fi

    info "rust not found — installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    source "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"

    if command -v rustc &>/dev/null; then
        ok "rust installed: $(rustc --version)"
    else
        err "rust installation failed — check https://rustup.rs"
    fi
}

# ── build ─────────────────────────────────────────────────

build_project() {
    info "building p2pfiletransfer (release mode)..."

    if ! command -v cargo &>/dev/null; then
        source "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"
    fi

    cargo build --release 2>&1

    if [ -f "$SCRIPT_DIR/target/release/p2pfiletransfer" ]; then
        ok "build complete: target/release/p2pfiletransfer"
    elif [ -f "$SCRIPT_DIR/target/release/p2pfiletransfer.exe" ]; then
        ok "build complete: target/release/p2pfiletransfer.exe"
    else
        err "build failed — binary not found"
    fi
}

# ── install to PATH (optional) ────────────────────────────

install_binary() {
    local dest

    case "$PLATFORM" in
        linux|macos)
            dest="$HOME/.local/bin"
            ;;
        windows)
            dest="$HOME/.cargo/bin"
            ;;
    esac

    echo ""
    read -rp "Install to $dest so you can run 'p2pfiletransfer' from anywhere? [y/N] " answer
    case "$answer" in
        [yY]|[yY][eE][sS])
            mkdir -p "$dest"
            if [ "$PLATFORM" = "windows" ]; then
                cp "$SCRIPT_DIR/target/release/p2pfiletransfer.exe" "$dest/"
            else
                cp "$SCRIPT_DIR/target/release/p2pfiletransfer" "$dest/"
                chmod +x "$dest/p2pfiletransfer"
            fi

            if echo "$PATH" | grep -q "$dest"; then
                ok "installed to $dest (already in PATH)"
            else
                warn "installed to $dest — add it to your PATH:"
                echo ""
                echo "  echo 'export PATH=\"$dest:\$PATH\"' >> ~/.bashrc"
                echo "  source ~/.bashrc"
                echo ""
            fi
            ;;
        *)
            info "skipped — run directly with: ./target/release/p2pfiletransfer"
            ;;
    esac
}

# ── run ───────────────────────────────────────────────────

install_deps
install_rust
build_project
install_binary

echo ""
echo "══════════════════════════════════════════"
echo "  setup complete!"
echo ""
echo "  quick start:"
echo "    ./start.sh send ./myfile.txt"
echo "    ./start.sh receive /ip4/.../p2p/..."
echo "══════════════════════════════════════════"
echo ""
