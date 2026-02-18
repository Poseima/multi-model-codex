#!/bin/bash
set -e

if [ -z "$1" ]; then
    echo "Usage: $0 <THREAD_ID>"
    echo "  Forks the given thread and runs the archive agent on the fork."
    echo "  Example: $0 019c6c7b-a1c7-75a2-bc35-55baf810c18a"
    exit 1
fi

ANCHOR_ID="$1"
MEMORY_DIR="$HOME/.dawn/.codex/memories_experiment/multi-model-codex"
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

echo "=== Archive Agent Test ==="

# Clean memory dir for fresh test
echo "Cleaning memory directory..."
rm -rf "$MEMORY_DIR/semantic/"*
rm -rf "$MEMORY_DIR/episodic/"*

# Build
echo "Building..."
cd "$REPO_DIR" && cargo build -p codex-exec

# Run archive on forked session (codex-exec has the resume --archive flag)
echo "Running archive on fork of thread $ANCHOR_ID..."
"$REPO_DIR/target/debug/codex-exec" resume "$ANCHOR_ID" --archive --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox

# Show results
echo ""
echo "=== Memory Files Created ==="
find "$MEMORY_DIR" -name "*.md" | sort
echo ""
echo "=== Semantic Files ==="
ls -la "$MEMORY_DIR/semantic/" 2>/dev/null || echo "(empty)"
echo ""
echo "=== Episodic Files ==="
ls -la "$MEMORY_DIR/episodic/" 2>/dev/null || echo "(empty)"
