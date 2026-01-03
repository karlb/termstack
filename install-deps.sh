#!/bin/bash
set -e

echo "Installing column-compositor dependencies..."

# Check if xwayland-satellite is installed
if ! command -v xwayland-satellite &> /dev/null; then
    echo "xwayland-satellite not found, installing from GitHub..."
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
echo "  ./target/release/column-compositor"
