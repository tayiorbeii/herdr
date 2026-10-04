#!/usr/bin/env bash
# shellcheck disable=SC2016 # "$task" and "$role" below are literal herdr token names
set -euo pipefail

usage() {
  cat <<'USAGE'
usage: scripts/demo/pane-styling-tour.sh [--herdr PATH] [--speed N] [--only IDS]
                                         [--pause] [--exit-when-done]

Self-running on-screen tour of the pane-styling config keys. The herdr client
takes over this terminal while a background driver edits the live config in a
vim pane, types captions and drives panes through the CLI. Saving in vim
reloads herdr (a BufWritePost autocmd set at the start). Each feature gets a
"## " header comment, its keys are typed, demonstrated and then commented out
with "# ", so every feature is seen against stock. The finale uncomments every
key at once, then comments them all out again.

When the tour ends the session stays live: the vim pane is focused on a config
that is stock but lists every feature key, commented, under its header. Delete
the "# " in front of a key and :w to try it; "reload" in the caption pane
reloads by hand. ctrl+b q exits, stops the session and removes the root.

Options:
  --herdr PATH      herdr binary (default: <repo>/target/debug/herdr)
  --speed N         delay divisor: 2 runs twice as fast, 0.5 half as fast (default 1)
  --only IDS        comma-separated subset, in stack order regardless of input:
                    t1 b1 fc1 u1 a1 gw1 s1 s2-pane s2-sidebar c1 c2 ib1 d1 all
  --pause           wait for Enter before starting (to start a recorder)
  --exit-when-done  stop the session and return the terminal when the tour
                    ends, instead of handing it over (unattended runs, recording)

Everything runs in a throwaway root with its own XDG_CONFIG_HOME/XDG_STATE_HOME
and session name; your config, state and running herdr servers are untouched.
ctrl+b q detaches and ends the tour early.

D1 (inactive dimming) needs a host terminal that answers OSC 10/11 color
queries (Ghostty, iTerm2, kitty, WezTerm; tmux with window-style set).
The popup (B1, U1) is opened through a linked local plugin's popup pane, which
shares the command-popup renderer; prefix+p opens the same popup by hand.
Requires: bash, POSIX userland, vim.
Set TOUR_MARK_FILE=PATH to log checkpoint names there (used for automated captures).
USAGE
}

herdr_arg=""
speed=1
only=""
pause=0
exit_when_done=0
while (($#)); do
  case "$1" in
    --herdr)
      herdr_arg="${2:?missing value for --herdr}"
      shift 2
      ;;
    --speed)
      speed="${2:?missing value for --speed}"
      shift 2
      ;;
    --only)
      only="${2:?missing value for --only}"
      shift 2
      ;;
    --pause)
      pause=1
      shift
      ;;
    --exit-when-done)
      exit_when_done=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

all_features="t1 b1 fc1 u1 a1 gw1 s1 s2-pane s2-sidebar c1 c2 ib1 d1 all"
selected="$all_features"
if [[ -n "$only" ]]; then
  wanted=" $(printf '%s' "$only" | tr 'A-Z,' 'a-z ') "
  selected=""
  for f in $wanted; do
    case " $all_features " in
      *" $f "*) ;;
      *) echo "unknown feature: $f (choose from: $all_features)" >&2; exit 2 ;;
    esac
  done
  for f in $all_features; do
    case "$wanted" in *" $f "*) selected="$selected $f" ;; esac
  done
fi

speed_pct="$(awk -v s="$speed" 'BEGIN { if (s + 0 > 0) printf "%d", s * 100; else print 0 }')"
if ((speed_pct < 1)); then
  echo "--speed must be a positive number (got $speed)" >&2
  exit 2
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "$script_dir/../.." && pwd)"
herdr_src="${herdr_arg:-$repo_dir/target/debug/herdr}"
if [[ ! -x "$herdr_src" ]]; then
  echo "herdr binary not found or not executable: $herdr_src" >&2
  echo "build it with 'cargo build' in the repo, or pass --herdr PATH" >&2
  exit 1
fi
vim_bin="$(command -v vim || true)"
if [[ -z "$vim_bin" ]]; then
  echo "vim is required for the editor pane" >&2
  exit 1
fi

utf8_locale=""
for l in "${LC_ALL:-}" "${LANG:-}" C.UTF-8 en_US.UTF-8 C.utf8 en_US.utf8; do
  case "$l" in
    *[Uu][Tt][Ff]-8*|*[Uu][Tt][Ff]8*)
      if locale -a 2>/dev/null | grep -qx "$l"; then utf8_locale="$l"; break; fi
      ;;
  esac
done
utf8_locale="${utf8_locale:-en_US.UTF-8}"
export LC_ALL="$utf8_locale"

while IFS= read -r var; do
  unset "$var"
done < <(compgen -e | grep '^HERDR_' || true)

session="tour$$"
root=""
driver_pid=""

cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  if [[ -n "$driver_pid" ]]; then kill "$driver_pid" 2>/dev/null || true; fi
  if [[ -n "$root" && -d "$root" ]]; then
    if [[ -s "$root/tour.pid" ]]; then kill "$(cat "$root/tour.pid")" 2>/dev/null || true; fi
    "$root/herdr" session stop "$session" >/dev/null 2>&1 || true
    if [[ -s "$root/error" ]]; then
      echo "tour aborted: $(cat "$root/error")" >&2
      ((rc)) || rc=1
    fi
    rm -rf "$root"
  fi
  [[ -t 0 ]] && stty sane 2>/dev/null
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# Unix socket paths are limited to ~104 bytes; fall back to /tmp for long TMPDIRs.
root="$(mktemp -d "${TMPDIR:-/tmp}/hpst.XXXXXX")"
if ((${#root} + ${#session} + 40 > 100)); then
  rmdir "$root"
  root="$(mktemp -d /tmp/hpst.XXXXXX)"
fi
mkdir -p "$root/c" "$root/s" "$root/home/herdr-demo" "$root/plugin"
cp "$herdr_src" "$root/herdr"
proj="$root/home/herdr-demo"
export XDG_CONFIG_HOME="$root/c" XDG_STATE_HOME="$root/s" HOME="$root/home"

# Debug builds use "herdr-dev", release builds "herdr": plant a broken file and see who reads it.
mkdir -p "$root/c/herdr-dev"
printf '[ui\n' >"$root/c/herdr-dev/config.toml"
app_dir=herdr
if ! "$root/herdr" config check >/dev/null 2>&1; then app_dir=herdr-dev; fi
cfg_dir="$root/c/$app_dir"
cfg="$cfg_dir/config.toml"
mkdir -p "$cfg_dir"

cat >"$root/demo-shell" <<EOF
#!/bin/sh
exec env -i HOME="$root/home" BASH_SILENCE_DEPRECATION_WARNING=1 TERM="\${TERM:-xterm-256color}" \\
  COLORTERM=truecolor LANG="$utf8_locale" LC_ALL="$utf8_locale" PATH="$(dirname "$vim_bin"):/usr/bin:/bin" \\
  PS1='demo\$ ' /bin/bash --norc --noprofile -i
EOF
chmod +x "$root/demo-shell"

cat >"$proj/colors.sh" <<'EOF'
printf 'default \033[31mred\033[0m \033[38;2;255;170;0mtruecolor\033[0m \033[2mfaint\033[0m \033[7mreverse\033[0m \033[44mon-blue\033[0m\n'
EOF
cat >"$proj/stream.sh" <<'EOF'
i=0; n=${1:-200}
while [ $i -lt "$n" ]; do
  printf 'build step %03d  \033[32mok\033[0m  compiling module_%02d\n' $i $((i % 37))
  i=$((i + 1)); sleep "${2:-0.08}"
done
EOF
cat >"$root/popup.sh" <<'EOF'
printf '\n  popup: default background\n  \033[41m explicit red background \033[0m stays red\n'
exec sleep "${TOUR_HOLD:-100000}"
EOF
ln -s "$cfg" "$proj/config.toml"

cat >"$root/plugin/herdr-plugin.toml" <<EOF
id = "tour.popup"
name = "Tour popup"
version = "0.1.0"
min_herdr_version = "0.6.10"
platforms = ["linux", "macos"]

[[panes]]
id = "popup"
title = "popup"
placement = "popup"
width = 46
height = 8
command = ["sh", "$root/popup.sh"]
EOF

cat >"$cfg" <<EOF
# herdr live config: the tour edits the two sections below
onboarding = false

[theme]
name = "catppuccin"

[theme.custom]

[ui]

# demo plumbing: isolation, clean shell, popup key
[experimental]
allow_nested = true
[update]
version_check = false
manifest_check = false
[terminal]
default_shell = "$root/demo-shell"
shell_mode = "non_login"
[[keys.command]]
key = "prefix+p"
type = "popup"
command = "sh $root/popup.sh"
description = "demo popup"
width = 46
height = 8
EOF
if ! "$root/herdr" config check >"$root/check.out" 2>&1; then
  echo "base config rejected by this herdr build:" >&2
  cat "$root/check.out" >&2
  exit 1
fi

# Reload hook for this session. The panes run a clean env (demo-shell), so it
# carries the isolated XDG dirs, the absolute binary and the session name.
# vim runs it on every :w; the caption pane's "reload" function runs it by
# hand. Its JSON lands in reload.out (atomically) so the driver can verify it.
cat >"$root/home/herdr-reload" <<EOF
#!/bin/sh
hr() { env XDG_CONFIG_HOME="$root/c" XDG_STATE_HOME="$root/s" "$root/herdr" "\$@"; }
out="\$(hr --session "$session" server reload-config 2>&1)"
printf '%s\n' "\$out" >"$root/reload.tmp" && mv -f "$root/reload.tmp" "$root/reload.out"
case "\$out" in
  *'"status":"applied"'*'"diagnostics":[]'*|*'"diagnostics":[]'*'"status":"applied"'*)
    printf 'herdr: config reloaded' ;;
  *)
    printf 'herdr: reload problem: %s' "\$(hr config check 2>&1 | head -n 2 | tr '\n' ' ')"
    exit 1 ;;
esac
EOF
chmod +x "$root/home/herdr-reload"

# --- driver: runs in the background while the client owns the terminal ---

h() { "$root/herdr" --session "$session" "$@"; }
die() { printf '%s\n' "$*" >"$root/error"; exit 1; }
mark() { if [[ -n "${TOUR_MARK_FILE:-}" ]]; then echo "$*" >>"$TOUR_MARK_FILE"; fi; }

nap() {
  local ms=$(($1 * 100 / speed_pct)) frac
  printf -v frac '%03d' $((ms % 1000))
  sleep "$((ms / 1000)).$frac"
}
hold() { nap 2600; }
beat() { nap 1200; }

first_id() { grep -o "\"$1\":\"[^\"]*\"" | head -1 | cut -d'"' -f4; }

wait_for() {
  local what="$1" tries=0
  shift
  until "$@" >/dev/null 2>&1; do
    tries=$((tries + 1))
    ((tries < 300)) || die "timed out waiting for $what"
    sleep 0.1
  done
}

type_into() {
  local pane="$1" text="$2" step="${3:-1}" ms="${4:-40}" i=0
  while ((i < ${#text})); do
    h pane send-text "$pane" "${text:i:step}" >/dev/null
    nap "$ms"
    i=$((i + step))
  done
}

caption() {
  h pane send-keys "$cap_pane" ctrl+l >/dev/null
  local line
  for line in "$@"; do
    type_into "$cap_pane" "$line" 1 35
    h pane send-keys "$cap_pane" Enter >/dev/null
  done
  beat
}
note() {
  type_into "$cap_pane" "$1" 2 35
  h pane send-keys "$cap_pane" Enter >/dev/null
}

vkeys() { h pane send-keys "$editor" "$@" >/dev/null; }
# Config lines and ex commands land in vim as whole lines (like a paste);
# only the caption pane uses the typewriter effect.
vtype() { h pane send-text "$editor" "$1" >/dev/null; nap 450; }
vcmd() { vtype "$1"; vkeys Enter; nap 250; }

file_changed_from() { [[ "$(cksum <"$cfg")" != "$1" ]]; }

# :w in vim; its BufWritePost autocmd reloads herdr through ~/herdr-reload.
# The driver still validates every step with `config check` and verifies the
# reload JSON that the hook leaves in reload.out.
save_and_reload() {
  local before
  before="$(cksum <"$cfg")"
  rm -f "$root/reload.out"
  vkeys Escape 0
  vcmd ':w'
  wait_for "vim to write config.toml" file_changed_from "$before"
  wait_for "the :w autocmd to reload herdr" test -s "$root/reload.out"
  if ! "$root/herdr" config check >"$root/check.out" 2>&1; then
    die "config check failed after an edit: $(tr '\n' ' ' <"$root/check.out")"
  fi
  grep -q '"status":"applied"' "$root/reload.out" || die "reload-config not applied: $(cat "$root/reload.out")"
  grep -q '"diagnostics":\[\]' "$root/reload.out" || die "reload-config diagnostics: $(cat "$root/reload.out")"
}

# append SECTION LINE...: type lines at the end of the section (before its
# blank line), save, reload. Set defer_save=1 to batch several edits.
defer_save=0
append() {
  local sect="$1" line
  shift
  vkeys Escape g g
  vcmd ":/^\\[$sect\\]/;/^\$/-1"
  vkeys o
  vtype "$1"
  shift
  for line in "$@"; do
    vkeys Enter
    vtype "$line"
  done
  ((defer_save)) || save_and_reload
}

# block SECTION HEADER LINE...: a "## " header comment, then the feature keys.
block() {
  local sect="$1" header="$2"
  shift 2
  append "$sect" "## $header" "$@"
}

# retype PREFIX LINE: replace the first active line starting with PREFIX (no save).
retype() {
  vkeys Escape g g
  vcmd "/^$1"
  vkeys c c
  vtype "$2"
  vkeys Escape
}
change() { retype "$@"; save_and_reload; }

# key_lines KEY...: line numbers of active (uncommented) lines setting KEY.
key_lines() {
  local re
  re="^($(IFS='|'; printf '%s' "$*")) = "
  awk -v re="$re" '$0 ~ re { print NR }' "$cfg"
}

# comment_keys KEY...: prefix the KEY lines with "# " on screen, one :s per
# run of consecutive line numbers, then save and reload. The buffer was just
# saved, so the file's line numbers are vim's.
comment_keys() {
  local -a nums=()
  local n start=0 prev=0
  while IFS= read -r n; do nums+=("$n"); done < <(key_lines "$@")
  ((${#nums[@]})) || die "no active lines to comment out for: $*"
  vkeys Escape
  for n in "${nums[@]}" 0; do
    if ((start && n == prev + 1)); then
      prev=$n
      continue
    fi
    if ((start == prev && start)); then
      vcmd ":${start}s/^/# /"
    elif ((start)); then
      vcmd ":${start},${prev}s/^/# /"
    fi
    start=$n prev=$n
  done
  save_and_reload
  note "#   ↺ commented out → stock"
}

# Feature lines live between [theme.custom] and the plumbing's [experimental].
feature_range=':/^\[theme.custom\]/;/^\[experimental\]/'
has_feature_lines() {
  awk -v want="$1" '/^\[theme\.custom\]$/ { on = 1 } /^\[experimental\]$/ { on = 0 }
    on && $0 ~ want { found = 1 } END { exit !found }' "$cfg"
}

seq_no=0
meta() {
  local pane="$1"
  shift
  seq_no=$((seq_no + 1))
  h pane report-metadata "$pane" --source demo --seq "$seq_no" "$@" >/dev/null
}

focus() {
  local src dir out
  case "$1" in
    d1) src="$editor" dir=right ;;
    d2) src="$d1" dir=down ;;
    d3) src="$d2" dir=down ;;
    editor) src="$cap_pane" dir=up ;;
  esac
  out="$(h pane focus --pane "$src" --direction "$dir")"
  [[ "$(printf '%s' "$out" | first_id focused_pane_id)" == "${!1}" ]] ||
    die "focus did not land on $1: $out"
}

popup() {
  local ms=$((2600 * 100 / speed_pct)) secs
  printf -v secs "%d.%03d" $((ms / 1000)) $((ms % 1000))
  h plugin pane open --plugin tour.popup --entrypoint popup --env "TOUR_HOLD=$secs" >/dev/null ||
    die "could not open the popup"
  nap 300
  mark "$1"
  sleep "$secs"
}

colors_all() {
  local p
  for p in "$d1" "$d2" "$d3"; do h pane run "$p" "clear; sh colors.sh" >/dev/null; done
}

setup_layout() {
  wait_for "the herdr server" h pane list
  editor="$(h pane list | first_id pane_id)"
  wait_for "the first shell prompt" h pane wait-output "$editor" --match 'demo$' --timeout 200
  d1="$(h pane split "$editor" --direction right --ratio 0.55 --no-focus | first_id pane_id)"
  d2="$(h pane split "$d1" --direction down --no-focus | first_id pane_id)"
  d3="$(h pane split "$d2" --direction down --no-focus | first_id pane_id)"
  cap_pane="$(h pane split "$editor" --direction down --ratio 0.78 --no-focus | first_id pane_id)"
  local tab1
  tab1="$(h pane list | first_id tab_id)"
  h tab rename "$tab1" tour >/dev/null
  tab_build="$(h tab create --label build --no-focus | first_id tab_id)"
  tab_review="$(h tab create --label review --no-focus | first_id tab_id)"
  h plugin link "$root/plugin" >/dev/null || die "could not link the popup plugin"
  local p
  for p in "$d1" "$d2" "$d3" "$cap_pane"; do
    wait_for "a shell prompt in $p" h pane wait-output "$p" --match 'demo$' --timeout 200
  done
  h pane run "$editor" "clear; vim -u NONE -N -n -i NONE -c 'syntax on' -c 'set nu nowrap ruler t_BE= ttimeoutlen=10' config.toml" >/dev/null
  wait_for "vim" h pane wait-output "$editor" --match 'onboarding = false' --timeout 200
  h pane run "$cap_pane" clear >/dev/null
  colors_all
  focus d1
  tab1_id="$tab1"
}

# The vim pane reloads herdr on every :w from here on (the driver keeps verifying).
setup_autoreload() {
  caption "# ▶ setup · saving the config reloads herdr" "#   a vim autocmd runs ~/herdr-reload on :w"
  vkeys Escape
  vcmd ':autocmd BufWritePost <buffer> redraw | echo system("~/herdr-reload")'
  beat
}

# Feature blocks: SECTION, "## " header, then single-line keys (one "# " toggles each).
block_t1() {
  block theme.custom 'T1 · active tab colors — focused tab label fg/bg' \
    'active_tab_fg = "#11111b"' 'active_tab_bg = "#fab387"'
}
block_b1() { block ui 'B1 · rounded borders — rounded corners on frames and popups' 'rounded_borders = true'; }
block_fc1() {
  block theme.custom 'FC1 · focus colors — focused / unfocused frame colors' \
    'pane_border_active = "#a6e3a1"' 'pane_border_inactive = "#45475a"'
}
block_u1() {
  block theme.custom 'U1 · popup colors — popup background and border' \
    'popup_bg = "#203040"' 'popup_border = "#ff8800"'
}
block_a1() { block ui 'A1 · focus weight — a focus cue that does not rely on color' 'pane_focus_weight = true'; }
block_gw1() { block ui 'GW1 · heavy borders — heavy lines on every pane frame' 'pane_heavy_borders = true'; }
block_s1() { block ui 'S1 · pane gap — blank cells between panes' "pane_gap_cells = ${1:-0}"; }
block_s2_pane() { block ui 'S2 · pane padding — inner padding; the terminal size follows' 'pane_padding_cells = 1'; }
block_s2_sidebar() { block ui 'S2 · sidebar padding — sidebar content inset' 'sidebar_padding_cells = 1'; }
block_c1() {
  block ui 'C1 · token titles — border titles from pane metadata tokens' \
    'pane_title_tokens = ["pane", { token = "$task", fg = "#f9e2af", bold = true }]'
}
block_c2() {
  block ui 'C2 · identity colors — unfocused frame color from a $role token' \
    'pane_border_identity_token = { token = "$role", rules = [{ equals = "build", fg = "#a6e3a1" }, { equals = "review", fg = "#89dceb" }] }'
}
block_ib1() { block theme.custom 'IB1 · inactive background — tint unfocused pane backgrounds' 'pane_inactive_bg = "#2a2a40"'; }
block_d1() { block ui 'D1 · inactive dimming — unfocused text blends toward the background' 'inactive_pane_dim_percent = 75'; }

feature_t1() {
  caption "# ▶ T1 · active tab colors" "#   only the focused tab label changes"
  block_t1
  mark "applied t1"; hold
  h tab focus "$tab_build" >/dev/null; beat
  h tab focus "$tab_review" >/dev/null; beat
  h tab focus "$tab1_id" >/dev/null; hold
  comment_keys active_tab_fg active_tab_bg
}

feature_b1() {
  caption "# ▶ B1 · rounded borders" "#   opt-in rounded corners on frames and popups"
  block_b1
  mark "applied b1"; hold
  note "#   the popup (also: ctrl+b p)"
  popup "popup b1"
  comment_keys rounded_borders
}

feature_fc1() {
  caption "# ▶ FC1 · focus colors" "#   separate focused / unfocused frame colors"
  block_fc1
  mark "applied fc1"; hold
  focus d2; beat
  focus d3; beat
  note "#   equal colors remove the color cue"
  change pane_border_active 'pane_border_active = "#6c7086"'
  change pane_border_inactive 'pane_border_inactive = "#6c7086"'
  mark "shown fc1"; hold
  focus d1
  note "#   distinct colors again, then comment out"
  retype pane_border_active 'pane_border_active = "#a6e3a1"'
  retype pane_border_inactive 'pane_border_inactive = "#45475a"'
  save_and_reload; beat
  comment_keys pane_border_active pane_border_inactive
}

feature_u1() {
  caption "# ▶ U1 · popup colors" "#   stock popup first"
  popup "popup u1-stock"
  block_u1
  mark "applied u1"
  note "#   new border + background; red line keeps red"
  popup "popup u1"
  comment_keys popup_bg popup_border
}

feature_a1() {
  caption "# ▶ A1 · focus weight" "#   a focus cue that does not rely on color"
  block_a1
  mark "applied a1"; hold
  focus d2; beat
  focus d3; beat
  focus d1; beat
  comment_keys pane_focus_weight
}

feature_gw1() {
  local n
  caption "# ▶ GW1 · heavy borders" "#   heavy lines on every pane frame"
  block_gw1
  mark "applied gw1"; hold
  note "#   + A1 focus weight: focused frame is double"
  n="$(awk '/^# pane_focus_weight = /{ print NR; exit }' "$cfg")"
  if [[ -n "$n" ]]; then
    vkeys Escape
    vcmd ":${n}s/^# //"
    save_and_reload
  else
    append ui 'pane_focus_weight = true'
  fi
  mark "shown gw1"; hold
  focus d2; beat
  focus d1; beat
  comment_keys pane_heavy_borders pane_focus_weight
}

feature_s1() {
  caption "# ▶ S1 · pane gap" "#   blank cells between panes: 0 → 2 → 4"
  block_s1
  mark "applied s1"; hold
  change pane_gap_cells 'pane_gap_cells = 2'
  hold
  change pane_gap_cells 'pane_gap_cells = 4'
  mark "shown s1"; hold
  retype pane_gap_cells 'pane_gap_cells = 2'
  comment_keys pane_gap_cells
}

feature_s2_pane() {
  caption "# ▶ S2 · pane padding" "#   inner padding; the terminal size follows (stty size)"
  h pane run "$d1" "clear; stty size" >/dev/null; beat
  block_s2_pane
  h pane run "$d1" "stty size" >/dev/null
  mark "applied s2-pane"; hold
  change pane_padding_cells 'pane_padding_cells = 2'
  h pane run "$d1" "stty size" >/dev/null
  mark "shown s2-pane"; hold
  retype pane_padding_cells 'pane_padding_cells = 1'
  comment_keys pane_padding_cells
  h pane run "$d1" "stty size" >/dev/null; beat
  colors_all
}

feature_s2_sidebar() {
  caption "# ▶ S2 · sidebar padding" "#   sidebar content inset; width unchanged"
  block_s2_sidebar
  mark "applied s2-sidebar"; hold
  change sidebar_padding_cells 'sidebar_padding_cells = 2'
  mark "shown s2-sidebar"; hold
  retype sidebar_padding_cells 'sidebar_padding_cells = 1'
  comment_keys sidebar_padding_cells
}

feature_c1() {
  caption "# ▶ C1 · token titles" "#   border titles from pane metadata tokens, live"
  meta "$d1" --token task=review-12
  meta "$d2" --token task=deploy-7
  block_c1
  mark "applied c1"; hold
  note "#   a tool reports new metadata"
  meta "$d2" --token task=deploy-8; hold
  note "#   short-lived value: TTL expiry falls back"
  local ttl=$((3000 * 100 / speed_pct))
  ((ttl >= 1500)) || ttl=1500
  meta "$d1" --token task=flaky-test --ttl-ms "$ttl"
  mark "shown c1"
  nap 600; sleep "$((ttl / 1000 + 1))"
  note "#   cleared token: plain title again"
  meta "$d2" --clear-token task; hold
  meta "$d1" --clear-token task
  comment_keys pane_title_tokens
}

feature_c2() {
  caption "# ▶ C2 · identity colors" "#   unfocused frame color from a \$role token"
  meta "$d2" --token role=build
  meta "$d3" --token role=review
  block_c2
  mark "applied c2"; hold
  note "#   role change, live"
  meta "$d3" --token role=build; hold
  note "#   cleared role → grey; focus always wins"
  meta "$d3" --clear-token role; beat
  focus d2
  mark "shown c2"; hold
  focus d1
  meta "$d2" --clear-token role
  comment_keys pane_border_identity_token
}

feature_ib1() {
  caption "# ▶ IB1 · inactive background" "#   tint only the default background of unfocused panes"
  block_ib1
  mark "applied ib1"; hold
  note "#   explicit colors untouched; tint follows focus"
  focus d2; beat
  focus d3; beat
  focus d1; hold
  comment_keys pane_inactive_bg
}

feature_d1() {
  caption "# ▶ D1 · inactive dimming" "#   unfocused text blends toward the background"
  block_d1
  mark "applied d1"; hold
  note "#   streaming output stays dimmed"
  h pane run "$d2" "clear; sh stream.sh 400" >/dev/null
  hold
  mark "shown d1"
  note "#   focus the stream: it brightens"
  focus d2; hold
  focus d1; beat
  note "#   (prefix mode, ctrl+b, pauses dimming)"
  h pane send-keys "$d2" ctrl+c >/dev/null
  comment_keys inactive_pane_dim_percent
  colors_all
}

feature_all() {
  local f
  caption "# ▶ everything together" "#   uncomment every key at once, then back to stock"
  meta "$d1" --token role=review --token task=review-12
  meta "$d2" --token role=build --token task=deploy-7
  if has_feature_lines '^# [a-z_]+ = '; then
    # Only "# key = ..." lines in the feature sections: "## " headers and the
    # plumbing comments never match.
    vkeys Escape g g
    vcmd "${feature_range}g/^# [a-z_]\\+ = /s/^# //"
    save_and_reload
  else
    # --only all: no feature blocks yet, so type them all (active) in one save.
    defer_save=1
    for f in t1 b1 fc1 u1 a1 gw1; do "block_$f"; done
    block_s1 2
    for f in s2_pane s2_sidebar c1 c2 ib1 d1; do "block_$f"; done
    defer_save=0
    save_and_reload
  fi
  mark "applied all"; hold
  note "#   stream + focus moves"
  h pane run "$d2" "clear; sh stream.sh 400" >/dev/null; beat
  focus d2; beat
  focus d3; beat
  focus d1; beat
  note "#   popup"
  popup "popup all"
  h pane send-keys "$d2" ctrl+c >/dev/null
  meta "$d1" --clear-token role --clear-token task
  meta "$d2" --clear-token role --clear-token task
}

ending() {
  caption "# ■ back to stock" "#   every key commented out = stock herdr"
  if has_feature_lines '^[a-z_]+ = '; then
    # Every active "key = ..." line between [theme.custom] and [experimental]:
    # only feature keys live there, so the result is stock-equivalent.
    vkeys Escape g g
    vcmd "${feature_range}g/^[a-z_]\\+ = /s/^/# /"
    save_and_reload
  fi
  colors_all
  mark "stock"; hold
}

# Leave the live session to the user: hints in the caption pane, a reload
# helper there, vim in normal mode on the first feature key, vim focused.
handover() {
  local line
  h pane send-keys "$cap_pane" ctrl+l >/dev/null
  type_into "$cap_pane" 'reload() { ~/herdr-reload; echo; }' 1 35
  h pane send-keys "$cap_pane" Enter >/dev/null
  for line in \
    '# ■ your turn: this herdr session stays live' \
    '# in vim, delete "# " in front of any key to re-enable' \
    '#   it, then :w (saving reloads herdr)' \
    '# headers starting with ## describe each feature' \
    '# reload (here) reloads herdr by hand' \
    '# ctrl+b q exits and cleans up'; do
    type_into "$cap_pane" "$line" 1 35
    h pane send-keys "$cap_pane" Enter >/dev/null
  done
  vkeys Escape g g
  vcmd '/^## '
  vkeys j 0
  focus editor
  mark "handover"
}

tour() {
  setup_layout
  mark "layout"
  hold
  setup_autoreload
  local f
  for f in $selected; do
    case "$f" in
      s2-pane) feature_s2_pane ;;
      s2-sidebar) feature_s2_sidebar ;;
      *) "feature_$f" ;;
    esac
  done
  ending
  if ((exit_when_done)); then
    hold
  else
    handover
  fi
}

driver() {
  local rc
  set +e
  (
    set -e
    tour
  ) &
  echo $! >"$root/tour.pid"
  wait $!
  rc=$?
  set -e
  if ((rc)) && [[ ! -s "$root/error" && ! -e "$root/detached" ]]; then
    printf 'driver failed (exit %s); see the last command in the driver log\n' "$rc" >"$root/error"
  fi
  mark "done rc=$rc"
  : >"$root/finished"
  # Default: the session stays up for the user; ctrl+b q ends it and the
  # EXIT trap stops it and removes the root.
  if ((rc)) || ((exit_when_done)); then
    "$root/herdr" session stop "$session" >/dev/null 2>&1 || true
  fi
}

if ((pause)); then
  read -r -p "Press Enter to start the tour (ctrl+b q ends it early)..." _ || true
fi

started=$SECONDS
driver >"$root/driver.log" 2>&1 &
driver_pid=$!
cd "$proj"
"$root/herdr" --session "$session" || true
if [[ -e "$root/finished" ]]; then
  wait "$driver_pid" 2>/dev/null || true
  driver_pid=""
  [[ -s "$root/error" ]] || echo "pane-styling tour finished; session closed after $((SECONDS - started))s"
else
  : >"$root/detached"
  if [[ -s "$root/tour.pid" ]]; then kill "$(cat "$root/tour.pid")" 2>/dev/null || true; fi
  wait "$driver_pid" 2>/dev/null || true
  driver_pid=""
  [[ -s "$root/error" ]] || echo "tour ended early: the client detached after $((SECONDS - started))s"
fi
