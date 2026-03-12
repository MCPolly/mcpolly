#!/usr/bin/env bash
#
# MCPolly Upgrade Script
# Safely upgrades a production MCPolly instance with zero-downtime aspirations:
#   - Backs up the database
#   - Downloads and verifies the new binary
#   - Gracefully stops the service
#   - Swaps the binary
#   - Restarts and health-checks the service
#
# Usage:
#   ./upgrade.sh                     # upgrade to latest release
#   MCPOLLY_VERSION=v0.3.0 ./upgrade.sh  # upgrade to a specific version
#
# Options (via env vars):
#   MCPOLLY_VERSION      - Target version (default: latest)
#   MCPOLLY_INSTALL_DIR  - Where the binary lives (default: auto-detect, fallback ~/.local/bin)
#   MCPOLLY_SERVICE      - Systemd service name (default: mcpolly)
#   MCPOLLY_SERVICE_TYPE - "user" or "system" (default: auto-detect)
#   MCPOLLY_URL          - Server URL for health checks (default: http://localhost:3000)
#   MCPOLLY_BACKUP_DIR   - Where to store DB backups (default: next to the database file)
#   MCPOLLY_DB           - Path to database file (default: auto-detect from service env)
#   MCPOLLY_SKIP_BACKUP  - Set to "1" to skip database backup
#   MCPOLLY_DRY_RUN      - Set to "1" to show what would happen without doing it
#
set -euo pipefail

REPO="MCPolly/mcpolly"
VERSION="${MCPOLLY_VERSION:-latest}"
SERVICE="${MCPOLLY_SERVICE:-mcpolly}"
SERVICE_TYPE="${MCPOLLY_SERVICE_TYPE:-}"
SERVER_URL="${MCPOLLY_URL:-http://localhost:3000}"
BACKUP_DIR="${MCPOLLY_BACKUP_DIR:-}"
DB_PATH="${MCPOLLY_DB:-}"
SKIP_BACKUP="${MCPOLLY_SKIP_BACKUP:-0}"
DRY_RUN="${MCPOLLY_DRY_RUN:-0}"
HEALTH_TIMEOUT=30
STOP_TIMEOUT=15

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

info()  { printf "${CYAN}▸${NC} %s\n" "$1"; }
ok()    { printf "${GREEN}✓${NC} %s\n" "$1"; }
warn()  { printf "${YELLOW}!${NC} %s\n" "$1"; }
fail()  { printf "${RED}✗${NC} %s\n" "$1" >&2; exit 1; }
dry()   { printf "${DIM}[dry-run]${NC} %s\n" "$1"; }
step()  { printf "\n${BOLD}── %s${NC}\n" "$1"; }

detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "darwin" ;;
        CYGWIN*|MINGW*|MSYS*) echo "windows" ;;
        *) fail "Unsupported OS: $(uname -s)" ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)   echo "x86_64" ;;
        aarch64|arm64)  echo "aarch64" ;;
        armv7l)         echo "armv7" ;;
        *) fail "Unsupported architecture: $(uname -m)" ;;
    esac
}

detect_libc() {
    if [ "$(detect_os)" != "linux" ]; then echo ""; return; fi
    if ldd --version 2>&1 | grep -qi musl || [ -f /etc/alpine-release ]; then
        echo "musl"
    else
        echo "gnu"
    fi
}

build_target() {
    local os="$1" arch="$2" libc="$3"
    case "${os}-${arch}" in
        linux-x86_64)   [ "$libc" = "musl" ] && echo "x86_64-unknown-linux-musl" || echo "x86_64-unknown-linux-gnu" ;;
        linux-aarch64)  [ "$libc" = "musl" ] && echo "aarch64-unknown-linux-musl" || echo "aarch64-unknown-linux-gnu" ;;
        linux-armv7)    echo "armv7-unknown-linux-gnueabihf" ;;
        darwin-x86_64)  echo "x86_64-apple-darwin" ;;
        darwin-aarch64) echo "aarch64-apple-darwin" ;;
        windows-x86_64) echo "x86_64-pc-windows-msvc" ;;
        *) fail "No prebuilt binary for ${os}/${arch}" ;;
    esac
}

resolve_version() {
    if [ "$VERSION" = "latest" ]; then
        info "Fetching latest release..."
        VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
        [ -z "$VERSION" ] && fail "Could not determine latest version"
    fi
    ok "Target version: ${VERSION}"
}

get_current_version() {
    # Try the running server's API first
    local resp
    resp=$(curl -sf "${SERVER_URL}/health" 2>/dev/null || true)
    if [ -n "$resp" ]; then
        local ver
        ver=$(curl -sf "${SERVER_URL}/api/v1/server/info" 2>/dev/null \
            | grep -o '"version":"[^"]*"' | head -1 | sed 's/"version":"//;s/"//' || true)
        if [ -n "$ver" ]; then echo "$ver"; return; fi
    fi
    echo "unknown"
}

detect_service_type() {
    if [ -n "$SERVICE_TYPE" ]; then return; fi
    if systemctl --user is-enabled "$SERVICE" &>/dev/null 2>&1; then
        SERVICE_TYPE="user"
    elif systemctl is-enabled "$SERVICE" &>/dev/null 2>&1; then
        SERVICE_TYPE="system"
    else
        SERVICE_TYPE="none"
    fi
}

systemctl_cmd() {
    if [ "$SERVICE_TYPE" = "user" ]; then
        systemctl --user "$@"
    elif [ "$SERVICE_TYPE" = "system" ]; then
        sudo systemctl "$@"
    fi
}

detect_install_dir() {
    if [ -n "${MCPOLLY_INSTALL_DIR:-}" ]; then return; fi

    # Try to find the binary from the running process
    local pid
    pid=$(pgrep -x mcpolly 2>/dev/null | head -1 || true)
    if [ -n "$pid" ]; then
        local exe
        exe=$(readlink -f "/proc/${pid}/exe" 2>/dev/null || true)
        if [ -n "$exe" ] && [ -f "$exe" ]; then
            MCPOLLY_INSTALL_DIR="$(dirname "$exe")"
            return
        fi
    fi

    # Try to find from systemd service ExecStart
    if [ "$SERVICE_TYPE" = "user" ]; then
        local exec_start
        exec_start=$(systemctl --user show "$SERVICE" -p ExecStart 2>/dev/null | sed 's/ExecStart=//;s/ .*//' || true)
        if [ -n "$exec_start" ] && [ -f "$exec_start" ]; then
            MCPOLLY_INSTALL_DIR="$(dirname "$exec_start")"
            return
        fi
    elif [ "$SERVICE_TYPE" = "system" ]; then
        local exec_start
        exec_start=$(systemctl show "$SERVICE" -p ExecStart 2>/dev/null | sed 's/ExecStart=//;s/ .*//' || true)
        if [ -n "$exec_start" ] && [ -f "$exec_start" ]; then
            MCPOLLY_INSTALL_DIR="$(dirname "$exec_start")"
            return
        fi
    fi

    # Try common locations
    for dir in "$HOME/.local/bin" "/opt/mcpolly" "/usr/local/bin"; do
        if [ -x "${dir}/mcpolly" ]; then
            MCPOLLY_INSTALL_DIR="$dir"
            return
        fi
    done

    MCPOLLY_INSTALL_DIR="$HOME/.local/bin"
    warn "Could not detect install dir, using default: ${MCPOLLY_INSTALL_DIR}"
}

detect_db_path() {
    if [ -n "$DB_PATH" ]; then return; fi

    # Try from systemd environment
    if [ "$SERVICE_TYPE" = "user" ]; then
        DB_PATH=$(systemctl --user show "$SERVICE" -p Environment 2>/dev/null \
            | grep -o 'DATABASE_URL=[^ ]*' | head -1 | sed 's/DATABASE_URL=//' || true)
    elif [ "$SERVICE_TYPE" = "system" ]; then
        DB_PATH=$(systemctl show "$SERVICE" -p Environment 2>/dev/null \
            | grep -o 'DATABASE_URL=[^ ]*' | head -1 | sed 's/DATABASE_URL=//' || true)
    fi

    # Try from running process environment
    if [ -z "$DB_PATH" ]; then
        local pid
        pid=$(pgrep -x mcpolly 2>/dev/null | head -1 || true)
        if [ -n "$pid" ]; then
            local cwd
            cwd=$(readlink -f "/proc/${pid}/cwd" 2>/dev/null || true)
            DB_PATH=$(tr '\0' '\n' < "/proc/${pid}/environ" 2>/dev/null \
                | grep '^DATABASE_URL=' | head -1 | sed 's/DATABASE_URL=//' || true)
            if [ -z "$DB_PATH" ] && [ -n "$cwd" ] && [ -f "${cwd}/mcpolly.db" ]; then
                DB_PATH="${cwd}/mcpolly.db"
            fi
        fi
    fi

    # Common fallback locations
    if [ -z "$DB_PATH" ]; then
        for candidate in \
            "${MCPOLLY_INSTALL_DIR}/mcpolly.db" \
            "/opt/mcpolly/data/mcpolly.db" \
            "$HOME/mcpolly.db" \
            "./mcpolly.db"; do
            if [ -f "$candidate" ]; then
                DB_PATH="$candidate"
                break
            fi
        done
    fi
}

verify_checksum() {
    local file="$1" asset_name="$2"
    local checksums_url="https://github.com/${REPO}/releases/download/${VERSION}/checksums.sha256"
    local checksums_file
    checksums_file="$(mktemp)"
    trap "rm -f '$checksums_file'" RETURN

    if ! curl -fsSL -o "$checksums_file" "$checksums_url" 2>/dev/null; then
        warn "Checksums file not available — skipping verification"
        return 0
    fi

    local expected
    expected=$(grep -F "  ${asset_name}" "$checksums_file" | head -1 | awk '{print $1}')
    if [ -z "$expected" ]; then
        warn "No checksum entry for ${asset_name} — skipping verification"
        return 0
    fi

    local actual
    if command -v sha256sum &>/dev/null; then
        actual=$(sha256sum "$file" | awk '{print $1}')
    elif command -v shasum &>/dev/null; then
        actual=$(shasum -a 256 "$file" | awk '{print $1}')
    else
        warn "No sha256sum or shasum found — skipping checksum verification"
        return 0
    fi

    if [ "$expected" = "$actual" ]; then
        ok "Checksum verified"
    else
        fail "Checksum mismatch! Expected: ${expected}, Got: ${actual}"
    fi
}

wait_for_stop() {
    local timeout="$1"
    info "Waiting for MCPolly to stop (timeout: ${timeout}s)..."
    local elapsed=0
    while [ $elapsed -lt "$timeout" ]; do
        if ! curl -sf "${SERVER_URL}/health" &>/dev/null; then
            ok "Service stopped"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    warn "Service did not stop within ${timeout}s"
    return 1
}

wait_for_healthy() {
    local timeout="$1"
    info "Waiting for MCPolly to become healthy (timeout: ${timeout}s)..."
    local elapsed=0
    while [ $elapsed -lt "$timeout" ]; do
        if curl -sf "${SERVER_URL}/health" &>/dev/null; then
            ok "Service is healthy"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    fail "Service did not become healthy within ${timeout}s — check logs with: journalctl --user -u ${SERVICE} -n 50"
}

rollback() {
    local backup_binary="$1" install_path="$2"
    warn "Rolling back to previous binary..."
    cp -f "$backup_binary" "$install_path"
    chmod +x "$install_path"
    if [ "$SERVICE_TYPE" != "none" ]; then
        systemctl_cmd start "$SERVICE"
    fi
    fail "Upgrade failed — rolled back to previous version"
}

main() {
    printf "\n${BOLD}MCPolly Upgrade${NC}\n\n"

    local os arch libc target
    os=$(detect_os)
    arch=$(detect_arch)
    libc=$(detect_libc)
    target=$(build_target "$os" "$arch" "$libc")

    ok "Platform: ${os}/${arch}${libc:+ (${libc})} → ${target}"

    detect_service_type
    ok "Service type: ${SERVICE_TYPE}"

    detect_install_dir
    local binary_path="${MCPOLLY_INSTALL_DIR}/mcpolly"
    if [ ! -x "$binary_path" ] && [ "$SERVICE_TYPE" = "none" ]; then
        # Also check if running from CWD
        if [ -x "./mcpolly" ]; then
            binary_path="./mcpolly"
            MCPOLLY_INSTALL_DIR="."
        elif [ -x "./target/release/mcpolly" ]; then
            binary_path="./target/release/mcpolly"
            MCPOLLY_INSTALL_DIR="./target/release"
        fi
    fi
    ok "Binary path: ${binary_path}"

    local current_version
    current_version=$(get_current_version "$binary_path")
    info "Current version: ${current_version}"

    resolve_version

    # ── Pre-flight checks ──

    step "Pre-flight checks"

    if [ ! -x "$binary_path" ] && [ "$DRY_RUN" != "1" ]; then
        fail "No mcpolly binary found at ${binary_path}. Set MCPOLLY_INSTALL_DIR or install first."
    fi

    detect_db_path
    if [ -n "$DB_PATH" ]; then
        ok "Database: ${DB_PATH}"
    else
        warn "Could not locate database file — backup will be skipped"
        SKIP_BACKUP=1
    fi

    # ── Download new binary ──

    step "Downloading ${VERSION}"

    local asset_name="mcpolly-${target}"
    [ "$os" = "windows" ] && asset_name="${asset_name}.exe"
    local download_url="https://github.com/${REPO}/releases/download/${VERSION}/${asset_name}"

    local tmpdir
    tmpdir="$(mktemp -d)"
    trap "rm -rf '$tmpdir'" EXIT

    local new_binary="${tmpdir}/${asset_name}"

    if [ "$DRY_RUN" = "1" ]; then
        dry "Would download: ${download_url}"
    else
        info "Downloading ${asset_name}..."
        local http_code
        http_code=$(curl -fsSL -w "%{http_code}" -o "$new_binary" "$download_url" 2>/dev/null || true)
        if [ ! -f "$new_binary" ] || [ "${http_code}" = "404" ]; then
            fail "Binary not found at ${download_url} — is ${VERSION} a valid release?"
        fi
        chmod +x "$new_binary"
        ok "Downloaded $(du -h "$new_binary" | awk '{print $1}')"

        verify_checksum "$new_binary" "$asset_name"
    fi

    # Also fetch mcpolly_mcp if it exists locally
    local mcp_binary_path="${MCPOLLY_INSTALL_DIR}/mcpolly_mcp"
    local new_mcp_binary=""
    if [ -x "$mcp_binary_path" ]; then
        local mcp_asset="mcpolly_mcp-${target}"
        [ "$os" = "windows" ] && mcp_asset="${mcp_asset}.exe"
        new_mcp_binary="${tmpdir}/${mcp_asset}"

        if [ "$DRY_RUN" = "1" ]; then
            dry "Would also download mcpolly_mcp"
        else
            info "Downloading ${mcp_asset} (MCP bridge)..."
            local mcp_url="https://github.com/${REPO}/releases/download/${VERSION}/${mcp_asset}"
            if curl -fsSL -o "$new_mcp_binary" "$mcp_url" 2>/dev/null; then
                chmod +x "$new_mcp_binary"
                verify_checksum "$new_mcp_binary" "$mcp_asset"
                ok "Downloaded mcpolly_mcp"
            else
                warn "mcpolly_mcp not available in this release — keeping current version"
                new_mcp_binary=""
            fi
        fi
    fi

    # ── Backup database ──

    if [ "$SKIP_BACKUP" != "1" ] && [ -n "$DB_PATH" ] && [ -f "$DB_PATH" ]; then
        step "Backing up database"

        local backup_dest="${BACKUP_DIR:-$(dirname "$DB_PATH")}"
        mkdir -p "$backup_dest"
        local timestamp
        timestamp=$(date +%Y%m%d_%H%M%S)
        local backup_file="${backup_dest}/mcpolly_backup_${timestamp}.db"

        if [ "$DRY_RUN" = "1" ]; then
            dry "Would backup ${DB_PATH} → ${backup_file}"
        else
            cp "$DB_PATH" "$backup_file"
            # Also copy WAL and SHM if they exist (SQLite journal files)
            [ -f "${DB_PATH}-wal" ] && cp "${DB_PATH}-wal" "${backup_file}-wal"
            [ -f "${DB_PATH}-shm" ] && cp "${DB_PATH}-shm" "${backup_file}-shm"
            local size
            size=$(du -h "$backup_file" | awk '{print $1}')
            ok "Backed up database (${size}) → ${backup_file}"

            # Prune old backups — keep last 5
            local backup_count
            backup_count=$(ls -1 "${backup_dest}"/mcpolly_backup_*.db 2>/dev/null | wc -l || echo 0)
            if [ "$backup_count" -gt 5 ]; then
                ls -1t "${backup_dest}"/mcpolly_backup_*.db | tail -n +6 | while read -r old; do
                    rm -f "$old" "${old}-wal" "${old}-shm"
                done
                ok "Pruned old backups (keeping last 5)"
            fi
        fi
    fi

    # ── Stop service ──

    step "Stopping MCPolly"

    local backup_binary="${tmpdir}/mcpolly.old"

    if [ "$DRY_RUN" = "1" ]; then
        dry "Would stop ${SERVICE} service"
    else
        # Save a copy of the old binary for rollback
        if [ -x "$binary_path" ]; then
            cp "$binary_path" "$backup_binary"
        fi

        if [ "$SERVICE_TYPE" != "none" ]; then
            systemctl_cmd stop "$SERVICE"
            wait_for_stop "$STOP_TIMEOUT" || true
        else
            # Try graceful kill of the process
            local pid
            pid=$(pgrep -x mcpolly 2>/dev/null | head -1 || true)
            if [ -n "$pid" ]; then
                info "Sending SIGTERM to pid ${pid}..."
                kill "$pid" 2>/dev/null || true
                wait_for_stop "$STOP_TIMEOUT" || {
                    warn "Sending SIGKILL..."
                    kill -9 "$pid" 2>/dev/null || true
                    sleep 1
                }
            else
                info "No running MCPolly process found"
            fi
        fi
    fi

    # ── Swap binary ──

    step "Installing new binary"

    if [ "$DRY_RUN" = "1" ]; then
        dry "Would replace ${binary_path}"
        [ -n "$new_mcp_binary" ] && dry "Would replace ${mcp_binary_path}"
    else
        cp -f "$new_binary" "$binary_path"
        chmod +x "$binary_path"
        ok "Replaced ${binary_path}"

        if [ -n "$new_mcp_binary" ]; then
            cp -f "$new_mcp_binary" "$mcp_binary_path"
            chmod +x "$mcp_binary_path"
            ok "Replaced ${mcp_binary_path}"
        fi
    fi

    # ── Start service ──

    step "Starting MCPolly"

    if [ "$DRY_RUN" = "1" ]; then
        dry "Would start ${SERVICE} service and health-check at ${SERVER_URL}/health"
    else
        if [ "$SERVICE_TYPE" != "none" ]; then
            systemctl_cmd start "$SERVICE"
            if wait_for_healthy "$HEALTH_TIMEOUT"; then
                :
            else
                rollback "$backup_binary" "$binary_path"
            fi
        else
            warn "No systemd service — start MCPolly manually"
            info "  PORT=3000 ${binary_path}"
        fi
    fi

    # ── Verify ──

    step "Verification"

    if [ "$DRY_RUN" = "1" ]; then
        dry "Would verify new version is running"
    else
        local new_running
        new_running=$(get_current_version "$binary_path")
        ok "Running version: ${new_running}"

        if [ "$SERVICE_TYPE" != "none" ]; then
            systemctl_cmd status "$SERVICE" --no-pager | head -5 || true
        fi
    fi

    # ── Done ──

    printf "\n${GREEN}${BOLD}Upgrade complete!${NC}\n"
    printf "  ${DIM}${current_version} → ${VERSION}${NC}\n"
    if [ -n "${backup_file:-}" ] && [ -f "${backup_file:-}" ]; then
        printf "  ${DIM}DB backup: ${backup_file}${NC}\n"
    fi
    printf "\n"
}

main "$@"
