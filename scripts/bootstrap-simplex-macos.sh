#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_dir="$root_dir/vendor/simplex-chat"
simplex_chat_revision="ec6e975001861d494360cda4aa267747d3a14272"
required_ghc="9.6.3"

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

brew install autoconf automake git libffi libtool openssl@3 pkg-config

# openssl@3 is keg-only in Homebrew, so Cabal's foreign-library check cannot
# discover libcrypto from the default compiler and pkg-config search paths.
homebrew_prefix="$(brew --prefix)"
openssl_prefix="$(brew --prefix openssl@3)"
export PATH="${homebrew_prefix}/bin:${HOME}/.ghcup/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export CPPFLAGS="-I${openssl_prefix}/include"
export LDFLAGS="-L${openssl_prefix}/lib"
export PKG_CONFIG_PATH="${openssl_prefix}/lib/pkgconfig"
unset PKG_CONFIG_LIBDIR
export CPATH="${openssl_prefix}/include"
export LIBRARY_PATH="${openssl_prefix}/lib"

# The pinned upstream packaging script otherwise downloads and compiles libffi
# from gitlab.haskell.org. Seed the cache it expects from Homebrew instead.
libffi_cache="/tmp/libffi-3.5.2/$(uname -m)-apple-darwin/.libs"
mkdir -p "$libffi_cache"
chmod u+w "$libffi_cache/libffi.dylib" 2>/dev/null || true
cp "$(brew --prefix libffi)/lib/libffi.dylib" "$libffi_cache/libffi.dylib"

if ! command -v ghcup >/dev/null 2>&1; then
  export BOOTSTRAP_HASKELL_NONINTERACTIVE=1
  export BOOTSTRAP_HASKELL_INSTALL_NO_STACK=1
  export BOOTSTRAP_HASKELL_INSTALL_NO_GHC=1
  export BOOTSTRAP_HASKELL_INSTALL_NO_CABAL=1
  curl --proto '=https' --tlsv1.2 -sSf https://get-ghcup.haskell.org | sh
fi
export PATH="${HOME}/.ghcup/bin:${PATH}"

ghc_path="$(ghcup whereis ghc "$required_ghc" 2>/dev/null || true)"
if [[ -z "$ghc_path" ]]; then
  ghcup install ghc "$required_ghc" --no-set
  ghc_path="$(ghcup whereis ghc "$required_ghc")"
elif ! "$ghc_path" --numeric-version >/dev/null 2>&1; then
  echo "GHC $required_ghc is installed but cannot run; reinstalling it..." >&2
  ghcup install ghc "$required_ghc" --no-set --force
  ghc_path="$(ghcup whereis ghc "$required_ghc")"
fi
if ! "$ghc_path" --numeric-version >/dev/null 2>&1; then
  echo "GHC $required_ghc still cannot run after installation." >&2
  exit 1
fi
if ! command -v cabal >/dev/null 2>&1; then
  ghcup install cabal --set
fi

if [[ ! -d "$source_dir/.git" ]]; then
  mkdir -p "$root_dir/vendor"
  git clone https://github.com/simplex-chat/simplex-chat.git "$source_dir"
fi
git -C "$source_dir" fetch origin stable
git -C "$source_dir" checkout --detach "$simplex_chat_revision"

cd "$source_dir"
ghcup run --ghc "$required_ghc" -- cabal update
ghcup run --ghc "$required_ghc" -- scripts/desktop/build-lib-mac.sh

upstream_arch="$(uname -m)"
[[ "$upstream_arch" == "arm64" ]] && upstream_arch="aarch64"
build_dir="$source_dir/apps/multiplatform/common/src/commonMain/cpp/desktop/libs/mac-$upstream_arch"
[[ -f "$build_dir/libsimplex.dylib" ]] || { echo "libsimplex.dylib was not produced" >&2; exit 1; }

install_dir="$root_dir/vendor/libsimplex"
mkdir -p "$install_dir"
cp "$build_dir"/*.dylib "$install_dir/"
# Upstream rewrites install names and rpaths after linking, invalidating the
# linker-provided signatures. Modern macOS kills the process during dlopen if
# any dylib in the bundle still has such a stale signature.
for library in "$install_dir"/*.dylib; do
  codesign --force --sign - "$library"
done
echo "Built $install_dir/libsimplex.dylib"
