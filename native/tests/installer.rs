//! One isolated shell-installer lifecycle check. No host installers or network.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

const STUB: &str = r#"#!/bin/bash
case "${0##*/}" in
  uname) echo Linux ;;
  nvim) echo 'NVIM v0.12.5' ;;
  rustc|cargo) [[ $1 == --version ]] || exit 1; echo 'rust 1.88.0' ;;
  tree-sitter) echo 'tree-sitter 0.26.10' ;;
  uv) echo 'uv 0.13.1' ;;
  apt-get)
    echo "apt-get $*" >> "$INSTALL_LOG"
    if [[ $1 == update && $FAIL_UPDATE == 1 ]]; then exit 1; fi
    if [[ $1 == install ]]; then /bin/ln -s "$0" "$TEST_BIN/git"; fi ;;
  sudo) exec "$@" ;;
  curl)
    echo curl >> "$INSTALL_LOG"
    echo 'echo installer-executed >> "$INSTALL_LOG"'
    exit 22 ;;
  sh|bash) echo installer-executed >> "$INSTALL_LOG"; exit 1 ;;
esac
"#;

#[test]
fn installer_requires_consent_rechecks_and_does_not_repeat_or_execute_failed_downloads() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let base = root.join(".tmp/installer");
    fs::create_dir_all(&base).unwrap();
    let temp = tempfile::tempdir_in(base).unwrap();
    let bin = temp.path().join("bin");
    let home = temp.path().join("home");
    let log = temp.path().join("install.log");
    fs::create_dir(&bin).unwrap();
    fs::create_dir(&home).unwrap();
    let tool = |name: &str| {
        let path = bin.join(name);
        fs::write(&path, STUB).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    for name in "uname nvim cargo rustc git curl tar cc c++ pkg-config tree-sitter uv magick apt-get sudo sh bash"
        .split_whitespace()
    {
        tool(name);
    }
    let actions = || fs::read_to_string(&log).unwrap_or_default();
    let run = |args: &[&str], fail_update: bool, status: i32| {
        let output = Command::new("/bin/bash")
            .arg(root.join("scripts/prerequisites.sh"))
            .args(args)
            .env_clear()
            .env("PATH", &bin)
            .env("HOME", &home)
            .env("TMPDIR", temp.path())
            .env("TEST_BIN", &bin)
            .env("INSTALL_LOG", &log)
            .env("FAIL_UPDATE", if fail_update { "1" } else { "0" })
            .env("NO_COLOR", "1")
            .current_dir(temp.path())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.status.code(), Some(status), "{args:?}\n{text}");
        assert!(!actions().contains("installer-executed"), "{}", actions());
        text
    };

    // No system Python exists in this PATH, and check mode must change nothing.
    run(&["--check"], false, 0);
    fs::remove_file(bin.join("uv")).unwrap();
    run(&["--check"], false, 1);
    assert!(actions().is_empty());
    tool("uv");

    fs::remove_file(bin.join("git")).unwrap();
    let preview = run(&["--install"], false, 1);
    assert!(preview.contains("apt-get install -y git"), "{preview}");
    assert!(actions().is_empty());
    run(&["--install", "--yes"], true, 1);
    assert_eq!(actions(), "apt-get update\n");
    fs::remove_file(&log).unwrap();

    let ready = run(&["--install", "--yes"], false, 0);
    assert!(ready.contains("Prerequisites ready"), "{ready}");
    let installed = actions();
    assert_eq!(installed, "apt-get update\napt-get install -y git\n");
    run(&["--install", "--yes"], false, 0);
    assert_eq!(actions(), installed);

    fs::remove_file(&log).unwrap();
    fs::remove_file(bin.join("uv")).unwrap();
    run(&["--install", "--yes"], false, 1);
    assert_eq!(actions(), "curl\n");
}
