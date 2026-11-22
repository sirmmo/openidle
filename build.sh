#!/bin/sh
# Test the engine, build the WebAssembly engine and the games' bundles into web/, build the
# command and the server. web/docs/ links to the docs.
set -e
cd "$(dirname "$0")"
cargo test --release --workspace
cargo build --profile wasm --target wasm32-unknown-unknown -p openidle-web
cp target/wasm32-unknown-unknown/wasm/openidle_web.wasm web/openidle.wasm
cargo build --release -p openidle-cli -p openidle-server
mkdir -p web/games
for g in games/*/; do
  ./target/release/openidle bundle "$g" -o "web/games/$(basename "$g").json"
done
echo "Play alone:   python3 -m http.server -d web 8000   then open http://localhost:8000/play/ (docs: http://localhost:8000/)"
echo "Play online:  OI_SITE=web ./target/release/openidle-server   then open http://localhost:8080/play/?world=main"
