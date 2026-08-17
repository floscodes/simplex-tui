#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_dir="$root_dir/vendor/simplex-chat"
simplex_chat_revision="ec6e975001861d494360cda4aa267747d3a14272"
required_ghc="9.6.3"

export PATH="${HOME}/.ghcup/bin:${PATH}"
for tool in gcc ld make pkg-config patchelf; do
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
  echo "SimpleX requires GHC $required_ghc; install it with: ghcup install ghc $required_ghc" >&2
  exit 1
fi

if [[ ! -d "$source_dir/.git" ]]; then
  mkdir -p "$root_dir/vendor"
  git clone https://github.com/simplex-chat/simplex-chat.git "$source_dir"
fi

git -C "$source_dir" fetch origin stable
git -C "$source_dir" checkout --detach "$simplex_chat_revision"

cd "$source_dir"
cabal update
ghcup run --ghc "$required_ghc" -- scripts/desktop/build-lib-linux.sh

arch="$(uname -m)"
build_dir="$(find "dist-newstyle/build/${arch}-linux" -path '*/simplex-chat-*/build/libsimplex.so' -printf '%h\n' | head -n 1)"
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
