#!/usr/bin/env bash
#
# MCPolly Installer
# Detects OS/architecture, downloads the correct binary from GitHub Releases,
# and prints the MCP configuration snippet.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/MCPolly/mcpolly/main/install.sh | bash
#
# Options (via env vars):
#   MCPOLLY_VERSION   - Version to install (default: latest)
#   MCPOLLY_INSTALL_DIR - Installation directory (default: ~/.local/bin)
#   MCPOLLY_BINARY    - Which binary to install: "both", "server", "mcp" (default: both)
#
set -euo pipefail

REPO="MCPolly/mcpolly"
VERSION="${MCPOLLY_VERSION:-latest}"
INSTALL_DIR="${MCPOLLY_INSTALL_DIR:-$HOME/.local/bin}"
BINARY="${MCPOLLY_BINARY:-both}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

info()  { printf "${CYAN}▸${NC} %s\n" "$1"; }
ok()    { printf "${GREEN}✓${NC} %s\n" "$1"; }
warn()  { printf "${YELLOW}!${NC} %s\n" "$1"; }
error() { printf "${RED}✗${NC} %s\n" "$1" >&2; exit 1; }

detect_os() {
    local os
    os="$(uname -s)"
    case "$os" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "darwin" ;;
        CYGWIN*|MINGW*|MSYS*) echo "windows" ;;
        FreeBSD*) echo "freebsd" ;;
        *) error "Unsupported operating system: $os" ;;
    esac
}

detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64)   echo "x86_64" ;;
        aarch64|arm64)  echo "aarch64" ;;
        armv7l)         echo "armv7" ;;
        *) error "Unsupported architecture: $arch" ;;
    esac
}

detect_libc() {
    if [ "$(detect_os)" != "linux" ]; then
        echo ""
        return
    fi
    if ldd --version 2>&1 | grep -qi musl; then
        echo "musl"
    elif [ -f /etc/alpine-release ]; then
        echo "musl"
    else
        echo "gnu"
    fi
}

resolve_version() {
    if [ "$VERSION" = "latest" ]; then
        info "Fetching latest release..."
        VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
        if [ -z "$VERSION" ]; then
            error "Could not determine latest version. Set MCPOLLY_VERSION explicitly."
        fi
    fi
    ok "Version: ${VERSION}"
}

build_asset_name() {
    local binary_name="$1"
    local os="$2"
    local arch="$3"
    local libc="$4"

    local target=""
    case "${os}-${arch}" in
        linux-x86_64)
            if [ "$libc" = "musl" ]; then
                target="x86_64-unknown-linux-musl"
            else
                target="x86_64-unknown-linux-gnu"
            fi
            ;;
        linux-aarch64)
            if [ "$libc" = "musl" ]; then
                target="aarch64-unknown-linux-musl"
            else
                target="aarch64-unknown-linux-gnu"
            fi
            ;;
        linux-armv7)
            target="armv7-unknown-linux-gnueabihf"
            ;;
        darwin-x86_64)
            target="x86_64-apple-darwin"
            ;;
        darwin-aarch64)
            target="aarch64-apple-darwin"
            ;;
        windows-x86_64)
            target="x86_64-pc-windows-msvc"
            ;;
        *)
            error "No prebuilt binary for ${os}/${arch}. Build from source: cargo build --release"
            ;;
    esac

    local ext=""
    if [ "$os" = "windows" ]; then
        ext=".exe"
    fi

    echo "${binary_name}-${target}${ext}"
}

download_binary() {
    local binary_name="$1"
    local asset_name="$2"
    local dest="$3"

    local url="https://github.com/${REPO}/releases/download/${VERSION}/${asset_name}"
    info "Downloading ${binary_name} from ${url}..."

    local http_code
    http_code=$(curl -fsSL -w "%{http_code}" -o "${dest}" "${url}" 2>/dev/null || true)

    if [ ! -f "${dest}" ] || [ "${http_code}" = "404" ]; then
        rm -f "${dest}"
        return 1
    fi

    chmod +x "${dest}"
    return 0
}

verify_checksum() {
    local file="$1"
    local asset_name="$2"
    local checksums_url="https://github.com/${REPO}/releases/download/${VERSION}/checksums.sha256"

    local checksums_file
    checksums_file="$(mktemp)"
    if curl -fsSL -o "${checksums_file}" "${checksums_url}" 2>/dev/null; then
        local expected
        expected=$(grep "${asset_name}" "${checksums_file}" | awk '{print $1}')
        if [ -n "$expected" ]; then
            local actual
            if command -v sha256sum &>/dev/null; then
                actual=$(sha256sum "$file" | awk '{print $1}')
            elif command -v shasum &>/dev/null; then
                actual=$(shasum -a 256 "$file" | awk '{print $1}')
            else
                warn "No sha256sum or shasum found — skipping checksum verification"
                rm -f "$checksums_file"
                return 0
            fi

            if [ "$expected" = "$actual" ]; then
                ok "Checksum verified"
            else
                rm -f "$checksums_file"
                error "Checksum mismatch! Expected: ${expected}, Got: ${actual}"
            fi
        else
            warn "No checksum entry found for ${asset_name} — skipping verification"
        fi
    else
        warn "Checksums file not available — skipping verification"
    fi
    rm -f "$checksums_file"
}

print_mcp_config() {
    local mcp_path="$1"
    local server_url="${MCPOLLY_URL:-http://localhost:3000}"

    printf "\n"
    printf "${BOLD}━━━ MCP Configuration ━━━${NC}\n"
    printf "\n"
    printf "Add to ${CYAN}~/.cursor/mcp.json${NC} (or your MCP client config):\n"
    printf "\n"

    if [ "$mcp_path" != "none" ]; then
        printf "${YELLOW}Option A: Stdio bridge (via installed binary)${NC}\n"
        cat <<STDIO
{
  "mcpServers": {
    "mcpolly": {
      "command": "${mcp_path}",
      "env": {
        "MCPOLLY_URL": "${server_url}",
        "MCPOLLY_API_KEY": "YOUR_API_KEY_HERE"
      }
    }
  }
}
STDIO
        printf "\n"
    fi

    printf "${YELLOW}Option B: Streamable HTTP (direct connection, no binary needed)${NC}\n"
    cat <<HTTP
{
  "mcpServers": {
    "mcpolly": {
      "url": "${server_url}/mcp",
      "headers": {
        "Authorization": "Bearer YOUR_API_KEY_HERE"
      }
    }
  }
}
HTTP

    printf "\n"
    printf "${BOLD}━━━ Next Steps ━━━${NC}\n"
    printf "\n"
    printf "  1. Start the MCPolly server:  ${CYAN}PORT=3000 ./mcpolly${NC}\n"
    printf "  2. Open the dashboard:        ${CYAN}http://localhost:3000${NC}\n"
    printf "  3. Copy the API key from the server log output\n"
    printf "  4. Replace YOUR_API_KEY_HERE in the config above\n"
    printf "\n"
}

main() {
    printf "\n${BOLD}MCPolly Installer${NC}\n\n"

    local os arch libc
    os=$(detect_os)
    arch=$(detect_arch)
    libc=$(detect_libc)

    ok "Detected: ${os}/${arch}${libc:+ (${libc})}"

    resolve_version

    mkdir -p "$INSTALL_DIR"

    local installed_server=false
    local installed_mcp=false
    local mcp_path="none"

    if [ "$BINARY" = "both" ] || [ "$BINARY" = "server" ]; then
        local server_asset
        server_asset=$(build_asset_name "mcpolly" "$os" "$arch" "$libc")
        local server_dest="${INSTALL_DIR}/mcpolly"
        [ "$os" = "windows" ] && server_dest="${server_dest}.exe"

        if download_binary "mcpolly" "$server_asset" "$server_dest"; then
            verify_checksum "$server_dest" "$server_asset"
            ok "Installed mcpolly server → ${server_dest}"
            installed_server=true
        else
            warn "mcpolly server binary not found for ${os}/${arch} in release ${VERSION}"
            warn "Build from source: cargo build --release --bin mcpolly"
        fi
    fi

    if [ "$BINARY" = "both" ] || [ "$BINARY" = "mcp" ]; then
        local mcp_asset
        mcp_asset=$(build_asset_name "mcpolly_mcp" "$os" "$arch" "$libc")
        local mcp_dest="${INSTALL_DIR}/mcpolly_mcp"
        [ "$os" = "windows" ] && mcp_dest="${mcp_dest}.exe"

        if download_binary "mcpolly_mcp" "$mcp_asset" "$mcp_dest"; then
            verify_checksum "$mcp_dest" "$mcp_asset"
            ok "Installed mcpolly_mcp bridge → ${mcp_dest}"
            installed_mcp=true
            mcp_path="$mcp_dest"
        else
            warn "mcpolly_mcp bridge binary not found for ${os}/${arch} in release ${VERSION}"
            warn "Build from source: cargo build --release --bin mcpolly_mcp"
        fi
    fi

    if ! $installed_server && ! $installed_mcp; then
        printf "\n"
        warn "No binaries were installed."
        warn "This likely means release ${VERSION} doesn't have prebuilt binaries for ${os}/${arch}."
        printf "\n"
        info "Build from source instead:"
        printf "  ${CYAN}git clone https://github.com/${REPO}.git${NC}\n"
        printf "  ${CYAN}cd mcpolly && cargo build --release${NC}\n"
        printf "\n"
        exit 1
    fi

    # Check if INSTALL_DIR is in PATH
    case ":$PATH:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            warn "${INSTALL_DIR} is not in your PATH"
            printf "  Add it:  ${CYAN}export PATH=\"${INSTALL_DIR}:\$PATH\"${NC}\n"
            printf "  Or add to your shell profile (~/.bashrc, ~/.zshrc, etc.)\n"
            ;;
    esac

    print_mcp_config "$mcp_path"

    ok "Installation complete!"
    printf "\n"
}

main "$@"
