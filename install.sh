#!/bin/sh
# Helix CLI Installer
# One-liner to download, install, and launch Helix.
# Usage: curl -fsSL https://raw.githubusercontent.com/poornachandra24/Helix/main/install.sh | sh

set -eu

# Color variables for a beautiful UX
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# Print banner
printf "${BLUE}╭────────────────────────────────────────────────────────╮${NC}\n"
printf "${BLUE}│${NC}    ${BOLD}${CYAN}Helix - Autonomous Tool-Calling Agent CLI${NC}           ${BLUE}│${NC}\n"
printf "${BLUE}├────────────────────────────────────────────────────────┤${NC}\n"
printf "${BLUE}│${NC}  Installing the latest precompiled static binary...    ${BLUE}│${NC}\n"
printf "${BLUE}╰────────────────────────────────────────────────────────╯${NC}\n\n"

# Check dependencies
for cmd in curl tar; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        printf "${RED}Error: Required command '$cmd' is not installed.${NC}\n" >&2
        exit 1
    fi
done

# Define repository details
REPO="poornachandra24/Helix"
GITHUB_API="https://api.github.com/repos/${REPO}/releases/latest"

# Detect OS
OS_TYPE=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$OS_TYPE" in
    linux*)   OS="unknown-linux-gnu" ;;
    darwin*)  OS="apple-darwin" ;;
    msys*|cygwin*|mingw*) OS="pc-windows-msvc" ;;
    *)
        printf "${RED}Error: Unsupported operating system: $OS_TYPE${NC}\n" >&2
        exit 1
        ;;
esac

# Detect Architecture
ARCH_TYPE=$(uname -m)
case "$ARCH_TYPE" in
    x86_64|amd64) ARCH="x86_64" ;;
    arm64|aarch64) ARCH="aarch64" ;;
    *)
        printf "${RED}Error: Unsupported architecture: $ARCH_TYPE${NC}\n" >&2
        exit 1
        ;;
esac

# Determine target asset name pattern
# Linux: helix-x86_64-unknown-linux-gnu.tar.gz or similar
# macOS: universal binary/archive or specific arch
# Let's map target triplet
# Determine target asset name pattern
TARGET_TRIPLET="${ARCH}-${OS}"

printf "${CYAN}• Detecting system:${NC} ${BOLD}${OS_TYPE} (${ARCH_TYPE})${NC} -> ${TARGET_TRIPLET}\n"

# Fetch latest release tag and download URL
printf "${CYAN}• Querying latest release from GitHub...${NC}\n"
LATEST_RELEASE_JSON=$(curl -s "$GITHUB_API")

# Check if rate limited or release not found
if echo "$LATEST_RELEASE_JSON" | grep -q "API rate limit exceeded"; then
    printf "${YELLOW}Warning: GitHub API rate limit exceeded. Falling back to downloading latest master tag.${NC}\n"
    # Fallback to redirect resolution
    DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/helix-${TARGET_TRIPLET}.tar.gz"
    TAG="latest"
elif echo "$LATEST_RELEASE_JSON" | grep -q '"message": "Not Found"'; then
    printf "${RED}Error: No releases have been published yet in this repository.${NC}\n" >&2
    printf "${YELLOW}Please tag and publish a release (e.g. tag v0.1.0 and push it) to trigger the release workflow first.${NC}\n" >&2
    exit 1
else
    # Parse tag name
    TAG=$(echo "$LATEST_RELEASE_JSON" | grep '"tag_name":' | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/')
    if [ -z "$TAG" ]; then
        printf "${RED}Error: Could not retrieve latest release tag.${NC}\n" >&2
        exit 1
    fi
    # Parse asset URL containing target triplet
    DOWNLOAD_URL=$(echo "$LATEST_RELEASE_JSON" | grep '"browser_download_url":' | grep "$TARGET_TRIPLET" | sed -E 's/.*"browser_download_url":[[:space:]]*"([^"]+)".*/\1/' | head -n 1)
    
    if [ -z "$DOWNLOAD_URL" ]; then
        # Fallback target pattern if tags match but files are structured differently
        DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/helix-${TARGET_TRIPLET}.tar.gz"
    fi
fi

printf "${CYAN}• Version:${NC} ${BOLD}${TAG}${NC}\n"
printf "${CYAN}• Downloading:${NC} ${DOWNLOAD_URL}\n"

# Create a temporary directory for extraction
TMP_DIR=$(mktemp -d)
clean_up() {
    rm -rf "$TMP_DIR"
}
trap clean_up EXIT INT TERM

# Download target archive
TARBALL="${TMP_DIR}/helix.tar.gz"
if ! curl -L --fail -o "$TARBALL" "$DOWNLOAD_URL"; then
    printf "${RED}Error: Failed to download release asset from ${DOWNLOAD_URL}${NC}\n" >&2
    exit 1
fi

# Extract archive
printf "${CYAN}• Extracting archive...${NC}\n"
tar -xzf "$TARBALL" -C "$TMP_DIR"

# Find binary
BINARY_PATH=""
if [ -f "${TMP_DIR}/helix" ]; then
    BINARY_PATH="${TMP_DIR}/helix"
elif [ -f "${TMP_DIR}/target/release/helix" ]; then
    BINARY_PATH="${TMP_DIR}/target/release/helix"
else
    # Search recursively for a file named helix (or helix.exe on windows)
    BINARY_PATH=$(find "$TMP_DIR" -type f \( -name "helix" -o -name "helix.exe" \) -print -quit)
fi

if [ -z "$BINARY_PATH" ] || [ ! -f "$BINARY_PATH" ]; then
    printf "${RED}Error: Could not find 'helix' binary in extracted archive.${NC}\n" >&2
    exit 1
fi

# Determine installation directory (default to ~/.local/bin)
INSTALL_DIR="${HOME}/.local/bin"
mkdir -p "$INSTALL_DIR"

printf "${CYAN}• Installing binary to:${NC} ${BOLD}${INSTALL_DIR}/helix${NC}\n"
cp "$BINARY_PATH" "${INSTALL_DIR}/helix"
chmod +x "${INSTALL_DIR}/helix"

printf "${GREEN}✓ Installation successful!${NC}\n\n"

# Path validation & environment check
PATH_IN_PATH=false
case ":$PATH:" in
    *:"$INSTALL_DIR":* | *:"$INSTALL_DIR/":*) PATH_IN_PATH=true ;;
esac

if [ "$PATH_IN_PATH" = false ]; then
    printf "${YELLOW}⚠️ Notice: '${INSTALL_DIR}' is not in your PATH.${NC}\n"
    printf "To run 'helix' from anywhere, add it to your shell configuration:\n\n"
    
    SHELL_PROFILE=""
    SHELL_NAME=$(basename "${SHELL:-}")
    if [ "$SHELL_NAME" = "zsh" ]; then
        SHELL_PROFILE="~/.zshrc"
    elif [ "$SHELL_NAME" = "bash" ]; then
        SHELL_PROFILE="~/.bashrc"
    else
        SHELL_PROFILE="your shell profile (~/.bashrc, ~/.zshrc, or ~/.profile)"
    fi
    
    printf "  echo 'export PATH=\"\$PATH:${INSTALL_DIR}\"' >> ${SHELL_PROFILE}\n"
    printf "  source ${SHELL_PROFILE}\n\n"
fi

# Run setup & REPL option
printf "${BOLD}Would you like to run Helix setup now? (y/n):${NC} "
# Read user confirmation (supports non-interactive pipes gracefully)
if [ -t 0 ]; then
    read -r RESPONSE < /dev/tty || RESPONSE="n"
else
    RESPONSE="n"
fi

case "$RESPONSE" in
    [yY][eE][sS]|[yY])
        printf "\n${CYAN}Starting Helix...${NC}\n\n"
        export PATH="$PATH:${INSTALL_DIR}"
        exec "${INSTALL_DIR}/helix"
        ;;
    *)
        printf "\nTo get started, simply run:\n"
        if [ "$PATH_IN_PATH" = false ]; then
            printf "  ${BOLD}${INSTALL_DIR}/helix${NC}\n"
        else
            printf "  ${BOLD}helix${NC}\n"
        fi
        printf "\nHave fun with your self-evolving CLI agent! 🚀\n"
        ;;
esac
