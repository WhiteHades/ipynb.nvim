#!/usr/bin/env bash
# Check host tools; uv manages Python, and :IpynbInstall installs the helper and engine.
set -eo pipefail

mode=check
images=1
native=1
yes=0
rust_toolchain=1.88.0
tree_version=0.26.10
for arg in "$@"; do
  case "$arg" in
    --check) mode=check ;;
    --install) mode=install ;;
    --no-images) images=0 ;;
    --python-backend) native=0 ;;
    --yes) yes=1 ;;
    -h|--help)
      printf '%s\n' 'Usage: bash scripts/prerequisites.sh [--check|--install] [--no-images] [--python-backend] [--yes]' \
        'Checks only by default. --install previews commands and asks before running them.' \
        '--yes accepts the install plan. Missing required tools produce exit status 1.' \
        '--python-backend skips native engine build tools; tree-sitter may still need Rust to install.' \
        'uv manages Python. :IpynbInstall installs the Python helper and Rust engine.'
      exit 0 ;;
    *) printf 'Unknown option: %s\n' "$arg" >&2; exit 2 ;;
  esac
done

# Install mode must see newly installed user tools without editing shell profiles.
if [[ $mode == install ]]; then
  export PATH="$HOME/.local/bin:$HOME/.cargo/bin${PATH:+:$PATH}"
fi

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
tool_ready() {
  local output
  has "$1" || return 1
  output=$("$1" --version 2>&1) || return 1
  version_at_least "$output" "$2" "$3" "$4"
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
  check_tool uv uv uv 'need 0.13.1+' 0 13 1
  row INFO Python 'managed by uv; no system Python required'
  if (( native )); then
    # The locked ICU 2.3 dependencies require Rust 1.88.
    check_tool Cargo cargo cargo 'need 1.88.0+' 1 88 0
    check_tool Rust rust rustc 'need 1.88.0+' 1 88 0
    check_tool pkg-config pkg-config pkg-config
    if has c++ || has g++ || has clang++; then row OK 'C++ compiler' available
    else missing+=(cxx); row NEED 'C++ compiler' 'not found'; fi
  fi
  check_tool Git git git
  check_tool curl curl curl
  check_tool tar tar tar
  if has cc || has gcc || has clang; then row OK 'C compiler' available
  else missing+=(cc); row NEED 'C compiler' 'not found'; fi
  check_tool tree-sitter tree-sitter tree-sitter 'need 0.26.1+' 0 26 1
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
packages=()
unsupported=()
need_uv=0
need_rust=0
need_tree=0
need_clt=0
add_package() {
  local existing
  for existing in "${packages[@]}"; do [[ $existing != "$1" ]] || return 0; done
  packages+=("$1")
}
system_package() {
  local package=$1 output
  case $manager in
    brew) add_package "$package"; return ;;
    pacman)
      output=$(LC_ALL=C pacman -Si "$package" 2>/dev/null) || return 1
      [[ $output =~ Version[[:space:]]*:[[:space:]]+([^[:space:]]+) ]] || return 1
      output=${BASH_REMATCH[1]} ;;
    apt)
      has apt-cache || return 1
      output=$(LC_ALL=C apt-cache policy "$package" 2>/dev/null) || return 1
      [[ $output =~ Candidate:[[:space:]]+([^[:space:]]+) ]] || return 1
      output=${BASH_REMATCH[1]} ;;
    dnf)
      output=$(dnf --cacheonly -q repoquery --latest-limit 1 --qf '%{version}' "$package" 2>/dev/null) || return 1 ;;
    *) return 1 ;;
  esac
  version_at_least "$output" "$2" "$3" "$4" || return 1
  add_package "$package"
}
for key in "${missing[@]}"; do
  package=$key
  case "$key" in
    uv)
      need_uv=1
      system_package uv 0 13 1 || :
      continue ;;
    cargo|rust)
      need_rust=1
      case $manager in
        apt) [[ $key != rust ]] || package=rustc ;;
        dnf) package=$key ;;
        *) package=rust ;;
      esac
      system_package "$package" 1 88 0 || :
      continue ;;
    tree-sitter)
      need_tree=1
      system_package tree-sitter-cli 0 26 1 || :
      continue ;;
  esac
  if [[ -z $manager ]]; then unsupported+=("$key"); continue; fi
  case "$manager:$key" in
    apt:cc|apt:cxx) package=build-essential ;;
    dnf:cc) package=gcc ;;
    dnf:cxx) package=gcc-c++ ;;
    pacman:cc|pacman:cxx) package=base-devel ;;
    brew:cc|brew:cxx) need_clt=1; continue ;;
    dnf:pkg-config) package=pkgconf-pkg-config ;;
    pacman:pkg-config|brew:pkg-config) package=pkgconf ;;
  esac
  add_package "$package"
done
if (( ${#unsupported[@]} )); then
  printf '\nNo supported package manager found. Install these system tools manually: %s\n' "${unsupported[*]}" >&2
  printf 'Then rerun --install to install missing user tools from official upstream sources.\n' >&2
  exit 1
fi
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
printf '\nInstall plan (%s):\n' "${manager:-official user-local installers}"
[[ $manager != apt || ${#packages[@]} == 0 ]] || show_command "${prefix[@]}" apt-get update
(( ${#command[@]} == 0 )) || show_command "${command[@]}"
if (( need_clt )); then show_command xcode-select --install; fi
if (( need_uv )); then
  printf '\n  If uv is still missing or too old, download and run Astral\047s official installer:\n'
  printf '  https://astral.sh/uv/install.sh\n'
  show_command env UV_NO_MODIFY_PATH=1 UV_INSTALL_DIR="$HOME/.local/bin" sh '<complete downloaded installer>'
fi
if (( need_rust || need_tree )); then
  printf '\n  If Rust is still missing or too old and needed for the engine or tree-sitter build:\n'
  if has rustup; then
    show_command rustup toolchain install "$rust_toolchain" --profile minimal
    show_command rustup default "$rust_toolchain"
  else
    printf '  Download and run the official Rust installer: https://sh.rustup.rs\n'
    show_command sh '<complete downloaded installer>' -y --no-modify-path --profile minimal --default-toolchain "$rust_toolchain"
  fi
  printf '  This selects Rust %s as the user\047s default only if existing Rust/Cargo are insufficient.\n' "$rust_toolchain"
fi
if (( need_tree )); then
  printf '\n  If the system package did not supply tree-sitter, build the pinned crates.io release:\n'
  show_command cargo install --locked --version "$tree_version" tree-sitter-cli
fi
printf '\nSystem packages may require sudo. Arch also upgrades the system.\n'
printf 'User installers do not edit shell profiles. This process uses ~/.local/bin and ~/.cargo/bin.\n'
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
if (( need_uv && ! failed )) && ! tool_ready uv 0 13 1; then
  installer=''
  if installer=$(curl --proto '=https' --tlsv1.2 -fsSL https://astral.sh/uv/install.sh) && [[ -n $installer ]]; then
    UV_NO_MODIFY_PATH=1 UV_INSTALL_DIR="$HOME/.local/bin" sh -c "$installer" || failed=1
  else
    printf 'Could not download the complete official uv installer.\n' >&2
    failed=1
  fi
  hash -r
  if ! tool_ready uv 0 13 1; then
    printf 'uv 0.13.1+ is still unavailable after installation.\n' >&2
    failed=1
  fi
fi
if (( need_tree )) && tool_ready tree-sitter 0 26 1; then need_tree=0; fi
if (( (need_rust || need_tree) && ! failed )) &&
    { ! tool_ready rustc 1 88 0 || ! tool_ready cargo 1 88 0; }; then
  if has rustup; then
    if rustup toolchain install "$rust_toolchain" --profile minimal; then
      rustup default "$rust_toolchain" || failed=1
    else failed=1; fi
  else
    installer=''
    if installer=$(curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs) && [[ -n $installer ]]; then
      sh -c "$installer" -- -y --no-modify-path --profile minimal --default-toolchain "$rust_toolchain" || failed=1
    else
      printf 'Could not download the complete official Rust installer.\n' >&2
      failed=1
    fi
  fi
  hash -r
  if ! tool_ready rustc 1 88 0 || ! tool_ready cargo 1 88 0; then
    printf 'Rust/Cargo 1.88.0+ are still unavailable after installation.\n' >&2
    failed=1
  fi
fi
if (( need_tree && ! failed )); then
  cargo install --locked --version "$tree_version" tree-sitter-cli || failed=1
fi
scan
if (( failed || ${#missing[@]} )); then
  printf '\nSetup is incomplete. Check errors above and your PATH.\n' >&2
  printf 'User tools live in ~/.local/bin and ~/.cargo/bin. Distribution packages may be too old.\n' >&2
  printf 'Neovim must be 0.12+: install an official matching release from https://github.com/neovim/neovim/releases\n' >&2
  printf 'ImageMagick must provide magick, not only ImageMagick 6 convert.\n' >&2
  exit 1
fi
printf '\nPrerequisites ready. Continue with the README plugin installation.\n'
