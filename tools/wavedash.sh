#!/bin/sh
# Builds OpenIdle's games for Wavedash (https://wavedash.com) and uploads each as a new build:
#   sh tools/wavedash.sh [game]     every game in wavedash/ (today: sparks), or one
# Each game is its own Wavedash game, configured in wavedash/<game>/wavedash.toml. This builds the
# engine and the bundles the way the Pages workflow does, then fills wavedash/<game>/build/ with
# the player (web/play/) flattened to the build's root: index.html, play.js, the engine
# (openidle.js, openidle.wasm) and that game's bundle, which the page opens without ?game=.
# No docs. With the `wavedash` CLI on PATH it then runs `wavedash build push` there
# (WAVEDASH_TOKEN from the environment); without it, or with WAVEDASH_DRY_RUN=1, it only says
# what it would push. Publishing a build is a separate step. The game id:
# WAVEDASH_GAME_ID_<GAME> (e.g. WAVEDASH_GAME_ID_SPARKS), else WAVEDASH_GAME_ID when pushing one
# game, else game_id in the game's wavedash.toml.
set -e
cd "$(dirname "$0")/.."
GAMES=$(cd wavedash && for g in */wavedash.toml; do printf '%s ' "${g%/wavedash.toml}"; done)
only=${1:-all}
case " all $GAMES " in
  *" $only "*) ;;
  *) echo "unknown game '$only': one of $GAMES, or all" >&2; exit 2 ;;
esac
# With one game, WAVEDASH_GAME_ID is its id.
set -- $GAMES
[ $# -gt 1 ] || only=$1

# Edits a file in place with sed (the same on GNU and BSD).
edit() { f=$1; shift; sed "$@" "$f" > "$f.tmp" && mv "$f.tmp" "$f"; }

# Every relative path the build's pages and scripts name must be in it; none may start at /.
check() {
  b=$1
  [ -f "$b/index.html" ] || { echo "$b/index.html is missing" >&2; exit 1; }
  bad=$(find "$b" -type f \( -name '*.html' -o -name '*.js' -o -name '*.css' \) ! -path '*/vendor/*' ! -name '*.min.js' |
    while read -r f; do
      d=$(dirname "$f")
      { grep -oE '(src|href)="[^"]*"' "$f" | sed -E 's/^(src|href)="//; s/"$//'
        grep -oE "(from|import\\() *['\"]\\.{1,2}/[^'\"]*['\"]" "$f" | sed -E "s/^(from|import\\() *['\"]//; s/['\"]\$//"
      } | while read -r r; do
        case $r in '' | '#'* | *'://'* | '//'* | data:* | mailto:* | javascript:* | *'$'* | *'{'*) continue ;; /*) echo "$f: $r starts at /"; continue ;; esac
        p=${r%%[?#]*}
        [ -z "$p" ] || [ -e "$d/$p" ] || echo "$f: $r is not in the build"
      done
    done)
  [ -z "$bad" ] || { echo "$bad" >&2; exit 1; }
}

# The variable that holds a game's id, and the id: game_id <game> <folder of its wavedash.toml>.
id_var() { echo "WAVEDASH_GAME_ID_$(echo "$1" | tr 'a-z-' 'A-Z_')"; }
game_id() {
  eval "id=\${$(id_var "$1"):-}"
  [ -n "$id" ] || [ "$only" = all ] || id=${WAVEDASH_GAME_ID:-}
  [ -n "$id" ] || id=$(sed -n 's/^game_id *= *"\([^"]*\)".*/\1/p' "$2/wavedash.toml")
  echo "$id"
}

# Uploads one build: push <game> <folder of its wavedash.toml> <its upload_dir>.
push() {
  slug=$1 conf=$2 build=$3
  check "$build"
  id=$(game_id "$slug" "$conf")
  msg=${WAVEDASH_MESSAGE:-"$slug $(git rev-parse --short HEAD 2>/dev/null || echo local)"}
  size=$(du -sh "$build" | cut -f1)
  if command -v wavedash >/dev/null 2>&1 && [ -z "${WAVEDASH_DRY_RUN:-}" ]; then
    case $id in '' | SET-ME) echo "$slug: no Wavedash game id: set $(id_var "$slug") or game_id in $conf/wavedash.toml" >&2; exit 1 ;; esac
    [ -n "${WAVEDASH_TOKEN:-}" ] || echo "$slug: WAVEDASH_TOKEN is not set; the CLI uses its own login, if any" >&2
    echo "$slug: pushing $build ($size) to Wavedash game $id"
    (cd "$conf" && WAVEDASH_GAME_ID=$id wavedash build push -m "$msg")
  else
    echo "$slug: would push $build ($size) to Wavedash game ${id:-SET-ME}: cd $conf && wavedash build push -m \"$msg\""
  fi
}

# As in the Pages workflow.
cargo build --profile wasm --target wasm32-unknown-unknown -p openidle-web
cp target/wasm32-unknown-unknown/wasm/openidle_web.wasm web/openidle.wasm
cargo build --release -p openidle-cli
mkdir -p web/games
for g in games/*/; do ./target/release/openidle bundle "$g" -o "web/games/$(basename "$g").json"; done

for g in $GAMES; do
  [ "$only" = all ] || [ "$only" = "$g" ] || continue
  [ -f "web/games/$g.json" ] || { echo "no games/$g/ to bundle" >&2; exit 1; }
  b=wavedash/$g/build
  rm -rf "$b"
  mkdir -p "$b/games"
  cp web/play/index.html web/play/play.js web/play/wavedash.js web/openidle.js web/openidle.wasm "$b/"
  cp "web/games/$g.json" "$b/games/"
  # The player sits one level down on the site; here it is the root, and opens this game.
  edit "$b/play.js" -e 's#\.\./#./#g' -e "s#q.get('game') || 'sparks'#q.get('game') || '$g'#"
  grep -q "q.get('game') || '$g'" "$b/play.js" || { echo "web/play/play.js no longer picks its game the way this script expects" >&2; exit 1; }
  push "$g" "wavedash/$g" "$b"
done
