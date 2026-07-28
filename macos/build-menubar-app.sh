#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Compatibility entry point. AICC now has one production macOS binary:
# the SwiftUI MenuBarExtra app built by build-aicc-swiftui.sh.
exec bash "$ROOT/macos/build-aicc-swiftui.sh"
