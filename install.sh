#!/bin/bash
# Rust Bot Installation Script (RustLink-Optimized)
# Server Files: /mnt/server

mkdir -p /mnt/server
cd /mnt/server

# Install native build dependencies required by RustLink.
#   cmake, build-essential, pkg-config  → needed by opus-sys (bundled libopus C build)
#   g++                                 → explicitly installs the C++ compiler + c++ symlink
#   libopus-dev                         → provides system opus, skips the bundled CMake build entirely
#   libssl-dev                          → only needed if you switch reqwest to default-tls (not recommended)
apt update
apt install -y cmake build-essential pkg-config g++ libopus-dev

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

export HOME=/mnt/server

# Pre-compile RustLink in release mode (unlimited installer timer)
if [ -f "/mnt/server/Cargo.toml" ]; then
    echo "Cargo.toml found! Checking for Cargo..."
    if command -v cargo &> /dev/null; then
        echo "Pre-compiling RustLink in release mode (This may take a few minutes)..."
        cargo build --release
        echo "Build complete! Make sure your egg's Startup Command points to ./target/release/rustlink"
    else
        echo "Cargo is not installed in the temporary installer container. Skipping pre-build."
    fi
fi

echo -e "Install complete!"
exit 0
