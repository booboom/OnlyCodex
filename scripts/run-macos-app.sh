#!/usr/bin/env bash
# Rebuild Python sidecar + Tauri .app, then open it for local macOS testing.
# Usage:
#   ./scripts/run-macos-app.sh
#   SKIP_SIDECAR=1 ./scripts/run-macos-app.sh   # only frontend/rust changed
#   NO_OPEN=1 ./scripts/run-macos-app.sh        # build only, do not launch
set -euo pipefail

project_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_dir"

log() {
	printf '\n==> %s\n' "$*"
}

die() {
	printf 'ERROR: %s\n' "$*" >&2
	exit 1
}

pick_python() {
	if [ -n "${PYTHON_BIN:-}" ]; then
		echo "$PYTHON_BIN"
		return 0
	fi

	candidates=(
		/opt/homebrew/bin/python3.11
		/opt/homebrew/bin/python3.12
		/opt/homebrew/bin/python3.13
		/usr/local/bin/python3.12
		/usr/local/bin/python3.11
	)

	# Prefer versioned binaries from PATH as well.
	if command -v python3.11 >/dev/null 2>&1; then
		candidates+=("$(command -v python3.11)")
	fi
	if command -v python3.12 >/dev/null 2>&1; then
		candidates+=("$(command -v python3.12)")
	fi
	if command -v python3 >/dev/null 2>&1; then
		candidates+=("$(command -v python3)")
	fi

	for candidate in "${candidates[@]}"; do
		if [ -n "$candidate" ] && [ -x "$candidate" ]; then
			if "$candidate" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 11) else 1)' 2>/dev/null; then
				echo "$candidate"
				return 0
			fi
		fi
	done

	die "Need Python 3.11+. Example: PYTHON_BIN=/opt/homebrew/bin/python3.11 $0"
}

app_name="OnlyCodex.app"
bundle_dir="$project_dir/src-tauri/target/release/bundle/macos"
app_path="$bundle_dir/$app_name"

command -v npm >/dev/null || die "npm not found"
command -v cargo >/dev/null || die "cargo not found"
command -v rustc >/dev/null || die "rustc not found"

if [ ! -d node_modules ]; then
	log "Installing npm dependencies"
	npm install
fi

if [ "${SKIP_SIDECAR:-0}" != "1" ]; then
	python_bin="$(pick_python)"
	log "Building Python sidecar with: ${python_bin}"
	PYTHON_BIN="${python_bin}" bash "$project_dir/scripts/build-sidecar.sh"
else
	log "Skipping sidecar build (SKIP_SIDECAR=1)"
	if [ ! -d "$project_dir/src-tauri/binaries" ]; then
		die "src-tauri/binaries missing; run without SKIP_SIDECAR once"
	fi
fi

log "Building macOS .app (release, app only)"
npm run tauri -- build --bundles app

if [ ! -d "$app_path" ]; then
	die "App not found: $app_path"
fi

if xattr -p com.apple.quarantine "$app_path" >/dev/null 2>&1; then
	log "Removing quarantine attribute"
	xattr -cr "$app_path" || true
fi

if [ "${NO_OPEN:-0}" = "1" ]; then
	log "Build finished (NO_OPEN=1, not launching)"
	printf '%s\n' "$app_path"
	exit 0
fi

log "Closing previous instance if running"
osascript -e 'tell application "OnlyCodex" to quit' >/dev/null 2>&1 || true
pkill -x "onlycodex-desktop" >/dev/null 2>&1 || true
sleep 0.5

log "Launching app"
open "$app_path"

printf '\nDone.\nApp path:\n  %s\n' "$app_path"
printf 'Next time:\n  ./scripts/run-macos-app.sh\n'
printf 'If only frontend/Rust changed:\n  SKIP_SIDECAR=1 ./scripts/run-macos-app.sh\n'
