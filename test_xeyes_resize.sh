#!/bin/bash
# Test script to diagnose X11 window resize issue

echo "=== X11 Window Resize Diagnostic Test ==="
echo ""
echo "This will:"
echo "1. Start the compositor"
echo "2. Wait for you to launch xeyes (run: xeyes)"
echo "3. Wait for you to try resizing the xeyes window"
echo "4. Show relevant log messages"
echo ""
echo "Press Ctrl+C when done testing"
echo ""

# Run compositor with detailed logging (including DEBUG for handle detection)
WINIT_UNIX_BACKEND=x11 \
RUST_LOG=column_compositor=info,compositor=debug \
./target/release/column-compositor 2>&1 | tee /tmp/resize_test.log

echo ""
echo "=== Test Complete ==="
echo ""
echo "Analyzing logs for resize events..."
echo ""

# Show resize-related messages
echo "--- Resize drag events ---"
grep "RESIZE DRAG" /tmp/resize_test.log || echo "No resize drag detected!"

echo ""
echo "--- External window resize requests ---"
grep "EXTERNAL WINDOW RESIZE" /tmp/resize_test.log || echo "No external window resize requests!"

echo ""
echo "--- X11 size hints ---"
grep "size hints" /tmp/resize_test.log || echo "No size hint checks!"

echo ""
echo "--- Fixed size warnings ---"
grep "FIXED SIZE\|BLOCKING RESIZE" /tmp/resize_test.log || echo "No fixed size warnings (good!)"

echo ""
echo "--- X11 configure requests ---"
grep "sending X11 configure" /tmp/resize_test.log || echo "No X11 configure requests sent!"

echo ""
echo "--- Configure notify responses ---"
grep "configure_notify" /tmp/resize_test.log | head -5 || echo "No configure_notify responses!"

echo ""
echo "--- Resize handle detection ---"
grep "RESIZE HANDLE FOUND\|find_resize_handle_at.*checking cell" /tmp/resize_test.log | tail -20 || echo "No handle detection attempts!"

echo ""
echo "Full log saved to: /tmp/resize_test.log"
echo "To see all handle detection attempts: grep 'find_resize_handle_at' /tmp/resize_test.log"
