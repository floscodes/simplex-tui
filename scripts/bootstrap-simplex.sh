#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_dir="$root_dir/vendor/simplex-chat"
simplex_chat_revision="ec6e975001861d494360cda4aa267747d3a14272"
required_ghc="9.6.3"
required_cabal="3.16.1.0"

install_system_dependencies() {
  if ! command -v apt-get >/dev/null 2>&1; then
    echo "Automatic dependency installation currently supports Debian/Ubuntu only." >&2
    echo "Install curl, git, gcc, make, binutils-gold, pkg-config, patchelf," >&2
    echo "libgmp-dev, libffi-dev, libncurses-dev and zlib1g-dev, then retry." >&2
    exit 1
  fi

  local packages=(
    build-essential binutils binutils-gold curl git libffi-dev libgmp-dev
    libncurses-dev patchelf pkg-config zlib1g-dev
  )
  local missing=()
  local package
  for package in "${packages[@]}"; do
    if ! dpkg-query -W -f='${Status}' "$package" 2>/dev/null | grep -q 'ok installed'; then
      missing+=("$package")
    fi
  done
  ((${#missing[@]} == 0)) && return

  local elevate=()
  if ((EUID != 0)); then
    if ! command -v sudo >/dev/null 2>&1; then
      echo "Installing system dependencies requires root or sudo." >&2
      exit 1
    fi
    elevate=(sudo)
  fi
  echo "Installing system dependencies: ${missing[*]}"
  "${elevate[@]}" apt-get update
  "${elevate[@]}" apt-get install -y "${missing[@]}"
}

install_ghcup() {
  if command -v ghcup >/dev/null 2>&1; then
    return
  fi
  echo "Installing ghcup..."
  export BOOTSTRAP_HASKELL_NONINTERACTIVE=1
  export BOOTSTRAP_HASKELL_INSTALL_NO_STACK=1
  export BOOTSTRAP_HASKELL_INSTALL_NO_GHC=1
  export BOOTSTRAP_HASKELL_INSTALL_NO_CABAL=1
  curl --proto '=https' --tlsv1.2 -sSf https://get-ghcup.haskell.org | sh
}

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "libsimplex bootstrap is currently supported on Linux only." >&2
  exit 1
fi

install_system_dependencies
install_ghcup

export PATH="${HOME}/.ghcup/bin:${PATH}"
for tool in curl gcc git ld make pkg-config patchelf; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing build tool: $tool" >&2
    echo "On Debian/Ubuntu install: build-essential binutils pkg-config patchelf" >&2
    exit 1
  fi
done
if [[ "$(gcc -print-prog-name=ld.gold)" == "ld.gold" ]] && ! command -v ld.gold >/dev/null 2>&1; then
  echo "Missing linker: ld.gold" >&2
  echo "On Debian/Ubuntu install: binutils-gold" >&2
  exit 1
fi
if ! ghcup whereis ghc "$required_ghc" >/dev/null 2>&1; then
  ghcup install ghc "$required_ghc" --no-set
fi
if ! ghcup whereis cabal "$required_cabal" >/dev/null 2>&1; then
  ghcup install cabal "$required_cabal" --no-set
fi

if [[ ! -d "$source_dir/.git" ]]; then
  mkdir -p "$root_dir/vendor"
  git clone https://github.com/simplex-chat/simplex-chat.git "$source_dir"
fi

git -C "$source_dir" fetch origin stable
git -C "$source_dir" checkout --detach "$simplex_chat_revision"

cd "$source_dir"
ghcup run --ghc "$required_ghc" --cabal "$required_cabal" -- cabal update
ghcup run --ghc "$required_ghc" --cabal "$required_cabal" -- scripts/desktop/build-lib-linux.sh

arch="$(uname -m)"
build_dir="$(find "dist-newstyle/build/${arch}-linux" -path '*/simplex-chat-*/build/libsimplex.so' -printf '%h\n' -quit)"
if [[ -z "$build_dir" ]]; then
  echo "libsimplex.so was not produced" >&2
  exit 1
fi

install_dir="$root_dir/vendor/libsimplex"
mkdir -p "$install_dir"
cp "$build_dir/libsimplex.so" "$install_dir/"
cp "$build_dir"/deps/*.so "$install_dir/"
for library in "$install_dir"/*.so; do
  patchelf --set-rpath '$ORIGIN' "$library"
done
echo "Built $install_dir/libsimplex.so"
