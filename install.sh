#!/bin/bash
# Rust Bot Installation Script (RustLink-Optimized)
# Server Files: /mnt/server

mkdir -p /mnt/server
cd /mnt/server

# Install native build dependencies required by RustLink.
#   cmake, build-essential, pkg-config  → needed by opus-sys (bundled libopus C build)
#   g++                                 → explicitly installs the C++ compiler + c++ symlink
#   libopus-dev                         → provides system opus, skips the bundled CMake build entirely
apt update
apt install -y cmake build-essential pkg-config g++ libopus-dev curl

# Safety net: ensure the `c++` symlink exists for CMake-based crates
if ! command -v c++ &> /dev/null && command -v g++ &> /dev/null; then
    ln -sf "$(which g++)" /usr/bin/c++
fi

# Verify pkg-config can find opus; if not, set the path manually
if ! pkg-config --exists opus 2>/dev/null; then
    echo "libopus-dev not found via pkg-config, checking common paths..."
    for dir in /usr/lib/pkgconfig /usr/lib/x86_64-linux-gnu/pkgconfig /usr/share/pkgconfig; do
        if [ -f "$dir/opus.pc" ]; then
            export PKG_CONFIG_PATH="$dir:${PKG_CONFIG_PATH:-}"
            echo "Found opus.pc in $dir"
            break
        fi
    done
fi

# User Upload protection
if [ "${USER_UPLOAD}" == "true" ] || [ "${USER_UPLOAD}" == "1" ]; then
    echo -e "User upload detected. Skipping git clone."
    exit 0
fi

## Add git ending if it's not on the address
if [[ ${GIT_ADDRESS} != *.git ]]; then
    GIT_ADDRESS=${GIT_ADDRESS}.git
fi

if [ -z "${USERNAME}" ] && [ -z "${ACCESS_TOKEN}" ]; then
    echo -e "Using anonymous git pull"
else
    GIT_ADDRESS="https://${USERNAME}:${ACCESS_TOKEN}@$(echo -e ${GIT_ADDRESS} | cut -d/ -f3-)"
fi

## Pull git repo
if [ "$(ls -A /mnt/server)" ]; then
    echo -e "/mnt/server directory is not empty."
    if [ -d .git ]; then
        if [ -f .git/config ]; then
            echo -e "Loading info from git config"
            ORIGIN=$(git config --get remote.origin.url)
            if [ "${ORIGIN}" == "${GIT_ADDRESS}" ]; then
                echo "Pulling latest from github"
                git pull
            fi
        else
            echo -e "Files found with no git config. Closing out to prevent breaking things."
            exit 10
        fi
    fi
else
    echo -e "/mnt/server is empty.\nCloning files into repo..."
    if [ -z "${BRANCH}" ]; then
        echo -e "Cloning default branch"
        git clone --depth 1 ${GIT_ADDRESS} .
    else
        echo -e "Cloning '${BRANCH}'"
        git clone --depth 1 --single-branch --branch ${BRANCH} ${GIT_ADDRESS} .
    fi
fi

# Pre-compile RustLink in release mode (unlimited installer timer)
# Set SKIP_COMPILE=true or PRE_COMPILE=false in the egg variables to skip.
if [ "${SKIP_COMPILE}" != "true" ] && [ "${PRE_COMPILE}" != "false" ] && [ -f "/mnt/server/Cargo.toml" ]; then
    echo "Cargo.toml found! Installing Rust toolchain for pre-compilation..."
    if ! command -v cargo &> /dev/null; then
        export HOME=/root
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source "/root/.cargo/env"
    fi
    if command -v cargo &> /dev/null; then
        export HOME=/mnt/server
        echo "Pre-compiling RustLink in release mode (Using single job to avoid OOM on low-RAM servers)..."
        if ! command -v c++ &> /dev/null && command -v g++ &> /dev/null; then
            export CXX=g++
        fi
        cargo build --release --jobs 1
        echo "Build complete! Make sure your egg's Startup Command points to ./target/release/rustlink"
    else
        echo "Failed to install Rust toolchain. Skipping pre-build."
    fi
else
    echo "Pre-compilation skipped (SKIP_COMPILE=${SKIP_COMPILE:-false}, PRE_COMPILE=${PRE_COMPILE:-true})."
fi

echo -e "Install complete!"
exit 0
