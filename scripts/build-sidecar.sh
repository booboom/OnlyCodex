#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "$0")/.." && pwd)"
python_bin="${PYTHON_BIN:-python3}"
target_triple="${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}"
venv_dir="$project_dir/.sidecar-venv"

"$python_bin" -c 'import sys; assert sys.version_info >= (3, 11), "Python 3.11+ is required to build the sidecar"'
if [ ! -x "$venv_dir/bin/pyinstaller" ]; then
	"$python_bin" -m venv "$venv_dir"
	"$venv_dir/bin/pip" install --disable-pip-version-check "pyinstaller>=6.0,<7"
fi

mkdir -p "$project_dir/src-tauri/binaries" "$project_dir/.sidecar-build"
"$venv_dir/bin/pyinstaller" --noconfirm --clean --onefile \
	--name opencodex-sidecar \
	--paths "$project_dir/python" \
	--distpath "$project_dir/.sidecar-build/dist" \
	--workpath "$project_dir/.sidecar-build/work" \
	--specpath "$project_dir/.sidecar-build" \
	"$project_dir/python/sidecar_main.py"

suffix=""
if [[ "$target_triple" == *windows* ]]; then suffix=".exe"; fi
cp "$project_dir/.sidecar-build/dist/opencodex-sidecar$suffix" \
	"$project_dir/src-tauri/binaries/opencodex-sidecar-$target_triple$suffix"
echo "Built OnlyCodex sidecar for $target_triple"
