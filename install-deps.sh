#!/bin/bash
set -e

echo "Installing termstack dependencies..."

# Check if running on Debian/Ubuntu
if command -v apt-get &> /dev/null; then
    echo ""
    echo "Checking system packages for xwayland-satellite..."

    # Check if libxcb-cursor-dev is installed
    if ! dpkg -l | grep -q libxcb-cursor-dev; then
        echo "Installing required system packages..."
        echo "This requires sudo access."
        sudo apt-get update
        sudo apt-get install -y libxcb-cursor-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
        echo "✓ System packages installed"
    else
        echo "✓ System packages already installed"
    fi
else
    echo ""
    echo "WARNING: Not a Debian/Ubuntu system. Please install these packages manually:"
    echo "  - libxcb-cursor-dev"
    echo "  - libxcb-render0-dev"
    echo "  - libxcb-shape0-dev"
    echo "  - libxcb-xfixes0-dev"
    echo ""
fi

# Check if xwayland-satellite is installed
if ! command -v xwayland-satellite &> /dev/null; then
    echo ""
    echo "xwayland-satellite not found, installing from GitHub..."
    echo "This may take a few minutes..."
    cargo install --git https://github.com/Supreeeme/xwayland-satellite.git xwayland-satellite
    echo "✓ xwayland-satellite installed"
else
    echo "✓ xwayland-satellite already installed"
fi

echo ""
echo "All dependencies installed successfully!"
echo ""
echo "To build the compositor:"
echo "  cargo build --release"
echo ""
echo "To run:"
echo "  ./target/release/termstack-compositor"
