#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_dir="$root_dir/vendor/simplex-chat"
simplex_chat_revision="ec6e975001861d494360cda4aa267747d3a14272"
required_ghc="9.6.3"
required_cabal="3.10.1.0"

[[ "$(uname -s)" == "Darwin" ]] || { echo "This bootstrap requires macOS." >&2; exit 1; }

if ! xcode-select -p >/dev/null 2>&1; then
  echo "Install Apple's Command Line Tools first: xcode-select --install" >&2
  exit 1
fi
if ! command -v brew >/dev/null 2>&1; then
  echo "Installing Homebrew..."
  NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  if [[ -x /opt/homebrew/bin/brew ]]; then
    eval "$(/opt/homebrew/bin/brew shellenv)"
  elif [[ -x /usr/local/bin/brew ]]; then
    eval "$(/usr/local/bin/brew shellenv)"
  fi
fi

brew install autoconf automake git libtool openssl@3 pkg-config

if ! command -v ghcup >/dev/null 2>&1; then
  export BOOTSTRAP_HASKELL_NONINTERACTIVE=1
  export BOOTSTRAP_HASKELL_INSTALL_NO_STACK=1
  export BOOTSTRAP_HASKELL_INSTALL_NO_GHC=1
  export BOOTSTRAP_HASKELL_INSTALL_NO_CABAL=1
  curl --proto '=https' --tlsv1.2 -sSf https://get-ghcup.haskell.org | sh
fi
export PATH="${HOME}/.ghcup/bin:${PATH}"

ghcup whereis ghc "$required_ghc" >/dev/null 2>&1 || ghcup install ghc "$required_ghc" --no-set
ghcup whereis cabal "$required_cabal" >/dev/null 2>&1 || ghcup install cabal "$required_cabal" --no-set

if [[ ! -d "$source_dir/.git" ]]; then
  mkdir -p "$root_dir/vendor"
  git clone https://github.com/simplex-chat/simplex-chat.git "$source_dir"
fi
git -C "$source_dir" fetch origin stable
git -C "$source_dir" checkout --detach "$simplex_chat_revision"

cd "$source_dir"
ghcup run --ghc "$required_ghc" --cabal "$required_cabal" -- cabal update
ghcup run --ghc "$required_ghc" --cabal "$required_cabal" -- scripts/desktop/build-lib-mac.sh

upstream_arch="$(uname -m)"
[[ "$upstream_arch" == "arm64" ]] && upstream_arch="aarch64"
build_dir="$source_dir/apps/multiplatform/common/src/commonMain/cpp/desktop/libs/mac-$upstream_arch"
[[ -f "$build_dir/libsimplex.dylib" ]] || { echo "libsimplex.dylib was not produced" >&2; exit 1; }

install_dir="$root_dir/vendor/libsimplex"
mkdir -p "$install_dir"
cp "$build_dir"/*.dylib "$install_dir/"
echo "Built $install_dir/libsimplex.dylib"
