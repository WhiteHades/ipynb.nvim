#!/usr/bin/env bash
# Check host tools; Python packages and the Rust engine are installed by :IpynbInstall.
set -eo pipefail

mode=check
images=1
yes=0
for arg in "$@"; do
  case "$arg" in
    --check) mode=check ;;
    --install) mode=install ;;
    --no-images) images=0 ;;
    --yes) yes=1 ;;
    -h|--help)
      printf '%s\n' 'Usage: bash scripts/prerequisites.sh [--check|--install] [--no-images] [--yes]' \
        'Checks only by default. --install previews commands and asks before running them.' \
        '--yes accepts the install plan. Missing required tools produce exit status 1.'
      exit 0 ;;
    *) printf 'Unknown option: %s\n' "$arg" >&2; exit 2 ;;
  esac
done

green='' yellow='' reset=''
if [[ -t 1 && -z ${NO_COLOR+x} && ${TERM:-dumb} != dumb ]]; then
  green=$'\033[32m'; yellow=$'\033[33m'; reset=$'\033[0m'
fi
row() {
  local color=$yellow
  [[ $1 != OK ]] || color=$green
  printf '  %s[%-4s]%s  %-22s %s\n' "$color" "$1" "$reset" "$2" "$3"
}
has() { command -v "$1" >/dev/null 2>&1; }
version_at_least() {
  local output=$1 major=$2 minor=$3 patch=$4
  [[ $output =~ ([0-9]+)\.([0-9]+)\.([0-9]+) ]] || return 1
  local a=${BASH_REMATCH[1]} b=${BASH_REMATCH[2]} c=${BASH_REMATCH[3]}
  (( a > major || (a == major && b > minor) || (a == major && b == minor && c >= patch) ))
}
check_tool() {
  local label=$1 key=$2 command=$3 minimum=${4:-} output=''
  if has "$command"; then
    if [[ -z $minimum ]]; then
      row OK "$label" "$(command -v "$command")"
      return
    fi
    output=$("$command" --version 2>&1) || output=''
    if version_at_least "$output" "${5}" "${6}" "${7}"; then
      row OK "$label" "${output%%$'\n'*}"
      return
    fi
  fi
  missing+=("$key")
  row NEED "$label" "${minimum:-not found}${output:+; found ${output%%$'\n'*}}"
}
scan() {
  missing=()
  printf '\nipynb.nvim | prerequisites\n\n'
  check_tool Neovim neovim nvim 'need 0.12.0+' 0 12 0
  check_tool Python python python3 'need 3.10.0+' 3 10 0
  check_tool Cargo cargo cargo 'need 1.85.0+' 1 85 0
  check_tool Rust rust rustc 'need 1.85.0+' 1 85 0
  check_tool Git git git
  check_tool curl curl curl
  check_tool tar tar tar
  if has cc || has gcc || has clang; then row OK 'C compiler' available
  else missing+=(cc); row NEED 'C compiler' 'not found'; fi
  if has c++ || has g++ || has clang++; then row OK 'C++ compiler' available
  else missing+=(cxx); row NEED 'C++ compiler' 'not found'; fi
  check_tool tree-sitter tree-sitter tree-sitter 'need 0.26.1+' 0 26 1
  if has uv; then row OK uv 'optional Python setup accelerator'
  else
    row INFO uv 'optional; using Python venv/pip'
    if python3 -c 'import venv, ensurepip' >/dev/null 2>&1; then
      row OK 'Python venv/pip' available
    else missing+=(venv); row NEED 'Python venv/pip' 'needed without uv'; fi
  fi
  if (( images )); then
    check_tool ImageMagick imagemagick magick
    case ${TERM_PROGRAM:-}:${TERM:-}:${KITTY_WINDOW_ID:-} in
      ghostty:*|kitty:*|*:xterm-kitty:*|*:*:[0-9]*) row OK 'Inline plot terminal' 'Kitty/Ghostty detected' ;;
      *) row INFO 'Inline plot terminal' 'use Kitty or Ghostty; detection is not conclusive through tmux/SSH' ;;
    esac
  fi
  printf '\n  %s prerequisite(s) need attention.\n' "${#missing[@]}"
}

manager=''
if [[ $(uname -s) == Darwin ]] && has brew; then manager=brew
elif has apt-get; then manager=apt
elif has dnf; then manager=dnf
elif has pacman; then manager=pacman
fi
scan
if [[ $mode == check ]] || (( ${#missing[@]} == 0 )); then
  (( ${#missing[@]} == 0 ))
  exit
fi
if [[ -z $manager ]]; then
  printf '\nNo supported package manager found. Install the NEED items manually.\n' >&2
  exit 1
fi

packages=()
need_tree=0
need_clt=0
for key in "${missing[@]}"; do
  package=$key
  case "$manager:$key" in
    *:tree-sitter) need_tree=1; continue ;;
    apt:python|apt:venv) package=python3-venv ;;
    dnf:python|dnf:venv) package=python3 ;;
    pacman:python|pacman:venv) package=python ;;
    brew:python|brew:venv) package=python ;;
    apt:rust|dnf:rust) package=rustc ;;
    pacman:cargo|pacman:rust|brew:cargo|brew:rust) package=rust ;;
    apt:cc|apt:cxx) package=build-essential ;;
    dnf:cc) package=gcc ;;
    dnf:cxx) package=gcc-c++ ;;
    pacman:cc|pacman:cxx) package=base-devel ;;
    brew:cc|brew:cxx) need_clt=1; continue ;;
  esac
  duplicate=0
  for existing in "${packages[@]}"; do [[ $existing != "$package" ]] || duplicate=1; done
  (( duplicate )) || packages+=("$package")
done
prefix=()
if [[ $manager != brew ]] && (( EUID != 0 )); then prefix=(sudo); fi
command=()
if (( ${#packages[@]} )); then
  case $manager in
    apt) command=("${prefix[@]}" apt-get install -y "${packages[@]}") ;;
    dnf) command=("${prefix[@]}" dnf install -y "${packages[@]}") ;;
    # A full upgrade avoids unsupported partial upgrades on Arch.
    pacman) command=("${prefix[@]}" pacman -Syu --needed --noconfirm "${packages[@]}") ;;
    brew) command=(brew install "${packages[@]}") ;;
  esac
fi
show_command() { printf '  '; printf '%q ' "$@"; printf '\n'; }
printf '\nInstall plan (%s):\n' "$manager"
[[ $manager != apt || ${#packages[@]} == 0 ]] || show_command "${prefix[@]}" apt-get update
(( ${#command[@]} == 0 )) || show_command "${command[@]}"
if (( need_clt )); then show_command xcode-select --install; fi
if (( need_tree )); then show_command cargo install --locked tree-sitter-cli; fi
printf '\nSystem packages may require sudo. Arch also upgrades the system.\n'
if (( ! yes )); then
  if [[ ! -t 0 ]]; then
    printf 'Rerun in a terminal, or use --install --yes to accept this plan.\n' >&2
    exit 1
  fi
  read -r -p 'Install these prerequisites? [y/N] ' answer
  [[ $answer == y || $answer == Y ]] || exit 1
fi
failed=0
if (( need_clt )); then
  xcode-select --install || failed=1
  printf 'Complete the Apple command-line tools installer, then rerun this script.\n'
  exit 1
fi
if (( ${#command[@]} )); then
  if [[ $manager == apt ]]; then "${prefix[@]}" apt-get update || failed=1; fi
  if (( ! failed )); then "${command[@]}" || failed=1; fi
fi
if (( need_tree && ! failed )); then
  cargo install --locked tree-sitter-cli || failed=1
fi
scan
if (( failed || ${#missing[@]} )); then
  printf '\nSetup is incomplete. Check errors above and your PATH.\n' >&2
  printf 'Cargo binaries usually live in ~/.cargo/bin. Distribution packages may be too old.\n' >&2
  printf 'For newer Neovim: https://github.com/neovim/neovim/releases\n' >&2
  printf 'For newer Rust: https://rustup.rs\n' >&2
  exit 1
fi
printf '\nPrerequisites ready. Continue with the README plugin installation.\n'
