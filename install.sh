#!/bin/sh
# agent-trace installer
# Usage: curl -fsSL https://raw.githubusercontent.com/Ray0907/agent-trace/refs/heads/main/install.sh | sh

set -eu

REPO="${AGENT_TRACE_REPO:-Ray0907/agent-trace}"
BINARY_NAME="agent-trace"
INSTALL_DIR="${AGENT_TRACE_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${AGENT_TRACE_VERSION:-}"
OS=""
ARCH=""
TARGET=""
TMP_DIR=""

info() {
    printf '[INFO] %s\n' "$1"
}

warn() {
    printf '[WARN] %s\n' "$1" >&2
}

error() {
    printf '[ERROR] %s\n' "$1" >&2
    exit 1
}

cleanup() {
    if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}

trap cleanup EXIT INT TERM

need_downloader() {
    if command -v curl >/dev/null 2>&1; then
        DOWNLOADER="curl"
        return
    fi

    if command -v wget >/dev/null 2>&1; then
        DOWNLOADER="wget"
        return
    fi

    error "curl or wget is required"
}

download_to_file() {
    url="$1"
    destination="$2"

    case "$DOWNLOADER" in
        curl)
            curl -fsSL "$url" -o "$destination"
            ;;
        wget)
            wget -qO "$destination" "$url"
            ;;
        *)
            error "unsupported downloader: $DOWNLOADER"
            ;;
    esac
}

download_to_stdout() {
    url="$1"

    case "$DOWNLOADER" in
        curl)
            curl -fsSL "$url"
            ;;
        wget)
            wget -qO- "$url"
            ;;
        *)
            error "unsupported downloader: $DOWNLOADER"
            ;;
    esac
}

detect_os() {
    case "$(uname -s)" in
        Darwin)
            OS="darwin"
            ;;
        Linux)
            OS="linux"
            ;;
        *)
            error "unsupported operating system: $(uname -s)"
            ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)
            ARCH="x86_64"
            ;;
        arm64|aarch64)
            ARCH="aarch64"
            ;;
        *)
            error "unsupported architecture: $(uname -m)"
            ;;
    esac
}

detect_target() {
    case "$OS:$ARCH" in
        darwin:x86_64)
            TARGET="x86_64-apple-darwin"
            ;;
        darwin:aarch64)
            TARGET="aarch64-apple-darwin"
            ;;
        linux:x86_64)
            TARGET="x86_64-unknown-linux-musl"
            ;;
        linux:aarch64)
            TARGET="aarch64-unknown-linux-gnu"
            ;;
        *)
            error "no release artifact for $OS/$ARCH"
            ;;
    esac
}

resolve_version() {
    if [ -n "$VERSION" ]; then
        return
    fi

    VERSION=$(
        download_to_stdout "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name":' \
        | sed -E 's/.*"([^"]+)".*/\1/' \
        | head -n 1
    )

    if [ -z "$VERSION" ]; then
        error "failed to resolve the latest release tag"
    fi
}

install_binary() {
    archive_name="${BINARY_NAME}-${TARGET}.tar.gz"
    download_url="https://github.com/${REPO}/releases/download/${VERSION}/${archive_name}"

    TMP_DIR=$(mktemp -d)
    archive_path="${TMP_DIR}/${archive_name}"

    info "Installing ${BINARY_NAME} ${VERSION}"
    info "Target: ${TARGET}"
    info "Download: ${download_url}"

    download_to_file "$download_url" "$archive_path" || error "failed to download release archive"

    tar -xzf "$archive_path" -C "$TMP_DIR" || error "failed to extract ${archive_name}"

    mkdir -p "$INSTALL_DIR"
    mv "${TMP_DIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
}

verify_install() {
    if "${INSTALL_DIR}/${BINARY_NAME}" --help >/dev/null 2>&1; then
        info "Installed ${INSTALL_DIR}/${BINARY_NAME}"
    else
        error "installed binary did not respond to --help"
    fi

    case ":$PATH:" in
        *":$INSTALL_DIR:"*)
            info "Run '${BINARY_NAME} serve' to start the local trace API"
            ;;
        *)
            warn "${INSTALL_DIR} is not on PATH"
            warn "Add it with: export PATH=\"${INSTALL_DIR}:\$PATH\""
            ;;
    esac
}

main() {
    need_downloader
    detect_os
    detect_arch
    detect_target
    resolve_version
    install_binary
    verify_install
}

main "$@"
