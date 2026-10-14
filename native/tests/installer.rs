//! Exercise the real shell entry point without using host installers or the network.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const STUB: &str = r#"#!/bin/bash
name=${0##*/}
case "$name:$1" in
  apt-cache:policy|dnf:--cacheonly|pacman:-Si)
    echo "$name $*" >> "$QUERY_LOG"
    case "${!#}" in
      uv) version=${REPO_UV_VERSION:-} ;;
      cargo|rust|rustc) version=${REPO_RUST_VERSION:-} ;;
      tree-sitter-cli) version=${REPO_TREE_VERSION:-} ;;
      *) exit 1 ;;
    esac
    [[ -n $version ]] || exit 1
    case "$name" in
      apt-cache) [[ ${LC_ALL:-} == C ]] || exit 1; printf '  Candidate: %s\n' "$version" ;;
      dnf) printf '%s\n' "$version" ;;
      pacman) [[ ${LC_ALL:-} == C ]] || exit 1; printf 'Version         : %s\n' "$version" ;;
    esac
    exit ;;
esac
case "$name" in
  uname) echo "${TEST_OS:-Linux}" ;;
  nvim) echo "NVIM v${NVIM_VERSION:-0.12.5}" ;;
  rustc|cargo)
    if [[ $1 == --version ]]; then echo "$name ${RUST_VERSION:-1.88.0}"
    else echo "$name $*" >> "$INSTALL_LOG"; exit "${INSTALL_STATUS:-0}"; fi ;;
  tree-sitter) echo "tree-sitter ${TREE_VERSION:-0.26.10}" ;;
  uv) echo "uv ${UV_VERSION:-0.13.1}" ;;
  apt-get|dnf|pacman|brew)
    case "$1" in
      info|-Si|list|search) exit 1 ;;
    esac
    echo "$name $*" >> "$INSTALL_LOG"
    if [[ $1 == update && ${FAIL_UPDATE:-0} == 1 ]]; then exit 1; fi
    if [[ $1 != update && ${INSTALL_STATUS:-0} != 0 ]]; then exit "$INSTALL_STATUS"; fi
    if [[ $1 == install && ${INSTALL_GIT:-0} == 1 ]]; then
      /bin/ln -s "$TEST_BIN/curl" "$TEST_BIN/git"
    fi
    if [[ ${INSTALL_USER_TOOLS:-0} == 1 ]]; then
      for tool in uv tree-sitter; do /bin/ln -s "$TEST_BIN/nvim" "$TEST_BIN/$tool"; done
    fi ;;
  sudo) exec "$@" ;;
  curl|wget)
    echo "$name $*" >> "$INSTALL_LOG"
    output=''
    while (( $# )); do
      case "$1" in -o|--output|-O) output=$2; shift ;; esac
      shift
    done
    payload='echo installer-executed >> "$INSTALL_LOG"'
    if [[ -n $output ]]; then printf '%s\n' "$payload" > "$output"
    else printf '%s\n' "$payload"; fi
    exit 22 ;;
  sh|bash|xcode-select)
    echo "installer-executed $name $*" >> "$INSTALL_LOG"; exit 1 ;;
esac
"#;

struct Host {
    _temp: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    home: PathBuf,
    log: PathBuf,
}

impl Host {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let base = root.join(".tmp/installer");
        fs::create_dir_all(&base).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("case-")
            .tempdir_in(base)
            .unwrap();
        let bin = temp.path().join("bin");
        let home = temp.path().join("home");
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&home).unwrap();
        let log = temp.path().join("install.log");
        let host = Self {
            _temp: temp,
            root,
            bin,
            home,
            log,
        };
        for name in [
            "uname",
            "nvim",
            "cargo",
            "rustc",
            "git",
            "curl",
            "wget",
            "tar",
            "cc",
            "c++",
            "pkg-config",
            "tree-sitter",
            "uv",
            "magick",
            "apt-get",
            "sudo",
            "sh",
            "bash",
            "xcode-select",
        ] {
            host.tool(name);
        }
        host
    }

    fn tool(&self, name: &str) {
        let path = self.bin.join(name);
        fs::write(&path, STUB).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn remove(&self, names: &[&str]) {
        for name in names {
            fs::remove_file(self.bin.join(name)).unwrap();
        }
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)], status: i32) -> String {
        let output: Output = Command::new("/bin/bash")
            .arg(self.root.join("scripts/prerequisites.sh"))
            .args(args)
            .env_clear()
            .env("PATH", &self.bin)
            .env("HOME", &self.home)
            .env("TMPDIR", self._temp.path())
            .env("INSTALL_LOG", &self.log)
            .env("QUERY_LOG", self._temp.path().join("queries.log"))
            .env("TEST_BIN", &self.bin)
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .envs(env.iter().copied())
            .current_dir(self._temp.path())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.status.code(),
            Some(status),
            "{args:?} {env:?}\n{text}"
        );
        assert!(!text.contains('\x1b'), "NO_COLOR was ignored: {text}");
        assert!(
            !self.actions().contains("installer-executed"),
            "failed downloads must never execute: {}",
            self.actions()
        );
        text
    }

    fn actions(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

#[test]
fn check_is_read_only_numeric_and_requires_uv_but_not_system_python() {
    let host = Host::new();
    host.run(&["--check"], &[], 0);
    // The fixture deliberately has neither python nor python3.
    host.run(
        &["--check"],
        &[("UV_VERSION", "0.14.10"), ("TREE_VERSION", "0.26.10")],
        0,
    );
    for (key, version, label) in [
        ("NVIM_VERSION", "0.11.9", "Neovim"),
        ("TREE_VERSION", "0.26.0", "tree-sitter"),
        ("UV_VERSION", "0.13.0", "uv"),
        ("RUST_VERSION", "1.87.9", "Rust"),
    ] {
        let text = host.run(&["--check"], &[(key, version)], 1);
        assert!(
            text.lines()
                .any(|line| line.contains("NEED") && line.contains(label)),
            "{text}"
        );
    }
    host.remove(&["uv"]);
    let text = host.run(&["--check"], &[], 1);
    assert!(!text.contains("optional"), "uv is required: {text}");
    assert!(host.actions().is_empty(), "check mode ran an installer");

    let host = Host::new();
    host.remove(&["cargo", "rustc", "pkg-config", "magick"]);
    host.run(&["--check", "--no-images"], &[], 1);
    host.run(&["--check", "--python-backend", "--no-images"], &[], 0);
    host.remove(&["cc", "c++", "tree-sitter"]);
    let text = host.run(&["--python-backend", "--no-images"], &[], 1);
    for label in ["C compiler", "tree-sitter"] {
        assert!(
            text.lines()
                .any(|line| line.contains("NEED") && line.contains(label)),
            "{text}"
        );
    }
    host.run(&["--bogus"], &[], 2);
    assert!(host.actions().is_empty());

    let host = Host::new();
    host.remove(&["magick"]);
    host.run(&[], &[], 1);
    host.run(&["--no-images"], &[], 0);
    assert!(host.actions().is_empty());
}

#[test]
fn apt_requires_consent_rechecks_failures_and_is_idempotent_when_ready() {
    let host = Host::new();
    host.remove(&["git"]);
    let text = host.run(&["--install"], &[], 1);
    assert!(text.contains("apt-get install -y git"), "{text}");
    assert!(host.actions().is_empty(), "preview ran an installer");
    host.remove(&["tree-sitter"]);
    host.run(&["--install", "--yes"], &[("FAIL_UPDATE", "1")], 1);
    assert_eq!(host.actions(), "apt-get update\n");
    host.tool("tree-sitter");
    fs::remove_file(&host.log).unwrap();
    host.run(&["--install", "--yes"], &[("INSTALL_STATUS", "1")], 1);
    assert_eq!(host.actions(), "apt-get update\napt-get install -y git\n");
    fs::remove_file(&host.log).unwrap();
    let text = host.run(&["--install", "--yes"], &[], 1);
    assert!(text.contains("Setup is incomplete"), "{text}");
    assert!(!text.contains("Prerequisites ready"), "{text}");
    fs::remove_file(&host.log).unwrap();
    let text = host.run(&["--install", "--yes"], &[("INSTALL_GIT", "1")], 0);
    assert!(text.contains("Prerequisites ready"), "{text}");
    let installed = host.actions();
    host.run(&["--install", "--yes"], &[], 0);
    assert_eq!(host.actions(), installed, "ready tools were reinstalled");

    let host = Host::new();
    let text = host.run(&["--install", "--yes"], &[("NVIM_VERSION", "0.11.9")], 1);
    assert!(text.contains("Setup is incomplete"), "{text}");
    assert!(!text.contains("Prerequisites ready"), "{text}");
}

#[test]
fn manager_plans_map_build_tools_and_tree_sitter_without_executing() {
    for (manager, os, plan, packages) in [
        (
            "apt-get",
            "Linux",
            "apt-get install -y",
            &["build-essential", "pkg-config"][..],
        ),
        (
            "dnf",
            "Linux",
            "dnf install -y",
            &["gcc", "gcc-c++", "pkgconf-pkg-config"][..],
        ),
        (
            "pacman",
            "Linux",
            "pacman -Syu --needed --noconfirm",
            &["base-devel", "pkgconf"][..],
        ),
        ("brew", "Darwin", "brew install", &["pkgconf"][..]),
    ] {
        let host = Host::new();
        host.remove(&["apt-get", "cc", "c++", "pkg-config"]);
        host.tool(manager);
        let text = host.run(&["--install"], &[("TEST_OS", os)], 1);
        assert!(text.contains(plan), "{manager}: {text}");
        for package in packages {
            assert!(
                text.contains(package),
                "{manager} missing {package}: {text}"
            );
        }
        if manager == "brew" {
            assert!(text.contains("xcode-select --install"), "{text}");
        }
        assert!(host.actions().is_empty(), "preview executed commands");
    }

    let host = Host::new();
    host.remove(&["apt-get", "git"]);
    let text = host.run(&["--install", "--yes"], &[], 1);
    assert!(text.contains("No supported package manager"), "{text}");
    assert!(host.actions().is_empty());

    let host = Host::new();
    host.remove(&["tree-sitter"]);
    let text = host.run(&["--install", "--yes"], &[], 1);
    assert!(text.contains("Setup is incomplete"), "{text}");
    assert!(
        host.actions().contains("cargo install --locked"),
        "{}",
        host.actions()
    );
    assert!(host.actions().contains("tree-sitter-cli"));
}

#[test]
fn missing_uv_uses_official_sources_and_failed_downloads_are_never_executed() {
    let host = Host::new();
    host.remove(&["uv"]);
    let text = host.run(&["--install"], &[], 1);
    assert!(text.contains("https://astral.sh/uv/install.sh"), "{text}");
    assert!(
        host.actions().is_empty(),
        "preview downloaded the installer"
    );
    let text = host.run(&["--install", "--yes"], &[], 1);
    assert!(
        host.actions().contains("https://astral.sh/uv/install.sh"),
        "{}",
        host.actions()
    );
    assert!(!text.contains("Prerequisites ready"), "{text}");
    assert!(
        !host.actions().contains("cargo install"),
        "uv must not use Cargo"
    );

    let host = Host::new();
    let text = host.run(&["--install", "--yes"], &[("UV_VERSION", "0.13.0")], 1);
    assert!(host.actions().contains("https://astral.sh/uv/install.sh"));
    assert!(!text.contains("Prerequisites ready"), "{text}");

    let host = Host::new();
    host.remove(&["rustc", "cargo"]);
    let text = host.run(&["--install", "--yes"], &[], 1);
    assert!(host.actions().contains("https://sh.rustup.rs"));
    assert!(!text.contains("Prerequisites ready"), "{text}");

    let host = Host::new();
    host.remove(&["apt-get", "uv", "tree-sitter"]);
    host.tool("pacman");
    let candidates = [
        ("REPO_UV_VERSION", "0.13.1"),
        ("REPO_TREE_VERSION", "0.26.1"),
    ];
    let text = host.run(&["--install"], &candidates, 1);
    assert!(text.contains("pacman -Syu --needed --noconfirm"), "{text}");
    assert!(
        text.contains(" uv") && text.contains("tree-sitter"),
        "{text}"
    );
    assert!(host.actions().is_empty());
    host.run(
        &["--install", "--yes"],
        &[
            ("REPO_UV_VERSION", "0.13.1"),
            ("REPO_TREE_VERSION", "0.26.1"),
            ("INSTALL_USER_TOOLS", "1"),
        ],
        0,
    );
    assert_eq!(
        host.actions(),
        "pacman -Syu --needed --noconfirm uv tree-sitter-cli\n"
    );
}

#[test]
fn cached_candidates_require_suitable_versions_without_refreshing_before_consent() {
    for (manager, plan, rust_package) in [
        ("apt-get", "apt-get install -y", "rustc"),
        ("dnf", "dnf install -y", "rust"),
        ("pacman", "pacman -Syu --needed --noconfirm", "rust"),
    ] {
        let host = Host::new();
        host.remove(&["apt-get", "uv", "cargo", "rustc", "tree-sitter"]);
        host.tool(manager);
        if manager == "apt-get" {
            host.tool("apt-cache");
        }
        for (uv, rust, tree, suitable) in [
            ("", "", "", false),
            ("0.13.0", "1.87.9", "0.26.0", false),
            ("0.13.1", "1.88.0", "0.26.1", true),
            ("0.14.10", "1.99.0", "0.26.10", true),
        ] {
            let text = host.run(
                &["--install"],
                &[
                    ("REPO_UV_VERSION", uv),
                    ("REPO_RUST_VERSION", rust),
                    ("REPO_TREE_VERSION", tree),
                ],
                1,
            );
            let command = text.lines().find(|line| line.contains(plan));
            if suitable {
                let command = command.unwrap_or_else(|| panic!("{manager}: {text}"));
                for package in ["uv", "cargo", rust_package, "tree-sitter-cli"] {
                    // pacman supplies both Cargo and rustc in its single rust package.
                    if manager != "pacman" || package != "cargo" {
                        assert!(
                            command.split_whitespace().any(|word| word == package),
                            "{command}"
                        );
                    }
                }
            } else {
                assert!(
                    command.is_none(),
                    "unsuitable cached versions queued installation: {text}"
                );
                assert!(!text.contains("apt-get update"), "{text}");
                assert!(text.contains("https://astral.sh/uv/install.sh"), "{text}");
            }
            assert!(
                host.actions().is_empty(),
                "preview performed a refresh, install, or download"
            );
        }
        let queries = fs::read_to_string(host._temp.path().join("queries.log")).unwrap();
        for package in ["uv", "cargo", rust_package, "tree-sitter-cli"] {
            if manager == "pacman" && package == "cargo" {
                continue;
            }
            let expected = match manager {
                "apt-get" => format!("apt-cache policy {package}"),
                "dnf" => format!(
                    "dnf --cacheonly -q repoquery --latest-limit 1 --qf %{{version}} {package}"
                ),
                _ => format!("pacman -Si {package}"),
            };
            assert!(queries.lines().any(|line| line == expected), "{queries}");
        }

        let host = Host::new();
        host.remove(&["apt-get"]);
        host.tool(manager);
        if manager == "apt-get" {
            host.tool("apt-cache");
        }
        let text = host.run(
            &["--install"],
            &[("UV_VERSION", "0.13.0"), ("REPO_UV_VERSION", "0.13.0")],
            1,
        );
        assert!(
            !text.contains(plan),
            "old uv alone must not queue a system upgrade: {text}"
        );
        assert!(host.actions().is_empty());
    }
}
