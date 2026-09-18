#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../android"
./gradlew --no-daemon "${1:-assembleDebug}"
