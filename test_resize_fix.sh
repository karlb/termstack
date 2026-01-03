#!/bin/bash
# Test script for X11 window resize fix

set -e

echo "=== Testing X11 Window Resize Fix ==="
echo ""

# Start compositor in background
echo "Starting compositor..."
WINIT_UNIX_BACKEND=x11 RUST_LOG=column_compositor=info ./target/release/column-compositor &
COMPOSITOR_PID=$!

# Give it time to start and create window
sleep 2

# Find the window
echo ""
echo "Looking for compositor window..."
WINDOW_ID=$(xdotool search --name "Column Compositor" 2>/dev/null | head -1)

if [ -z "$WINDOW_ID" ]; then
    echo "ERROR: Could not find compositor window"
    kill $COMPOSITOR_PID 2>/dev/null || true
    exit 1
fi

echo "Found window ID: $WINDOW_ID"

# Check WM_NORMAL_HINTS
echo ""
echo "=== WM_NORMAL_HINTS (size hints) ==="
xprop -id "$WINDOW_ID" WM_NORMAL_HINTS

# Get current geometry
echo ""
echo "=== Initial Geometry ==="
xwininfo -id "$WINDOW_ID" | grep -E "(Width|Height|geometry)"

# Try to resize the window
echo ""
echo "=== Attempting Resize ==="
echo "Resizing to 1600x1000..."
xdotool windowsize "$WINDOW_ID" 1600 1000
sleep 0.5

# Check new geometry
echo ""
echo "=== Geometry After Resize ==="
xwininfo -id "$WINDOW_ID" | grep -E "(Width|Height|geometry)"

echo ""
echo "=== Test Complete ==="
echo "If the Width/Height changed, resize is working!"
echo "If they stayed the same, resize is still broken."

# Cleanup
kill $COMPOSITOR_PID 2>/dev/null || true
