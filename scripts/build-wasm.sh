#!/usr/bin/env bash
# Builds the WASM engine for the web app.
#
#   scripts/build-wasm.sh            single-threaded build -> web/src/wasm/pkg
#   scripts/build-wasm.sh --threads  also the threaded + SIMD build -> web/src/wasm/pkg-mt
#                                    (needs a nightly toolchain with rust-src)
#   scripts/build-wasm.sh --dev      unoptimised single-threaded build
set -euo pipefail
cd "$(dirname "$0")/.."

profile=release
threads=0
for arg in "$@"; do
  case "$arg" in
    --dev) profile=dev ;;
    --threads) threads=1 ;;
    *) echo "unknown option $arg" >&2; exit 2 ;;
  esac
done
dir=$([[ $profile == dev ]] && echo debug || echo release)

cargo build -p overact-wasm --target wasm32-unknown-unknown --profile "$profile"
wasm-bindgen --target web --out-dir web/src/wasm/pkg "target/wasm32-unknown-unknown/$dir/overact_wasm.wasm"

if [[ $threads == 1 ]]; then
  # Shared memory needs a maximum; 4 GiB is the wasm32 limit.
  flags="-C target-feature=+atomics,+bulk-memory,+mutable-globals,+simd128"
  for a in --shared-memory --import-memory --max-memory=4294967296 \
    --export=__wasm_init_tls --export=__tls_size --export=__tls_align --export=__tls_base; do
    flags="$flags -C link-arg=$a"
  done
  RUSTFLAGS="$flags" \
  CARGO_TARGET_DIR=target/mt \
    rustup run nightly cargo build -p overact-wasm --features parallel \
      --target wasm32-unknown-unknown --profile "$profile" -Z build-std=panic_abort,std
  wasm-bindgen --target web --out-dir web/src/wasm/pkg-mt "target/mt/wasm32-unknown-unknown/$dir/overact_wasm.wasm"
  # wasm-bindgen-rayon's worker helper imports the package directory.
  printf '{"name":"overact-wasm-mt","type":"module","main":"overact_wasm.js","module":"overact_wasm.js","sideEffects":["./snippets/*"]}\n' > web/src/wasm/pkg-mt/package.json
fi

if [[ $threads == 0 && ! -f web/src/wasm/pkg-mt/overact_wasm_bg.wasm ]]; then
  # No threaded build: a stub so the app still bundles; it falls back to one thread.
  mkdir -p web/src/wasm/pkg-mt
  cat > web/src/wasm/pkg-mt/overact_wasm.js <<'STUB'
// Stub written by scripts/build-wasm.sh: build with --threads for the real one.
export default async function init() {
  throw new Error('threaded engine not built')
}
export function initThreadPool() {}
export function thread_count() { return 1 }
export function inspect_table() { throw new Error('threaded engine not built') }
export class WasmEngine {}
STUB
fi

if command -v wasm-opt >/dev/null && [[ $profile == release ]]; then
  for p in web/src/wasm/pkg/overact_wasm_bg.wasm web/src/wasm/pkg-mt/overact_wasm_bg.wasm; do
    [[ -f $p ]] && wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-threads --enable-simd -o "$p" "$p"
  done
fi
ls -la web/src/wasm/pkg*/*.wasm
