//! Filesystem discovery following jupyter_core.paths and KernelSpecManager.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn env_flag(name: &str) -> Option<bool> {
    env::var(name).ok().map(|value| {
        !matches!(
            value.to_lowercase().as_str(),
            "no" | "n" | "false" | "off" | "0" | "0.0"
        )
    })
}

fn owned(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        // SAFETY: geteuid takes no arguments and has no preconditions.
        fs::metadata(path).is_ok_and(|metadata| metadata.uid() == unsafe { geteuid() })
    }
    #[cfg(not(unix))]
    {
        // shortcut: no Windows ownership API in std, use the explicit override for shared envs.
        fs::metadata(path).is_ok_and(|metadata| !metadata.permissions().readonly())
    }
}

fn helper_prefix(python: &Path, cwd: &Path, search: &OsStr) -> Result<PathBuf> {
    let executable = if python
        .parent()
        .is_none_or(|path| path.as_os_str().is_empty())
    {
        env::split_paths(search)
            .map(|directory| {
                let mut path = cwd.join(directory).join(python);
                if cfg!(windows) && path.extension().is_none() {
                    path.set_extension("exe");
                }
                path
            })
            .find(|path| {
                fs::metadata(path).is_ok_and(|metadata| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                    }
                    #[cfg(not(unix))]
                    {
                        metadata.is_file()
                    }
                })
            })
            .with_context(|| format!("helper Python not found on PATH: {}", python.display()))?
    } else {
        cwd.join(python)
    };
    let directory = executable
        .parent()
        .context("helper Python has no parent directory")?;
    let prefix = if directory.file_name().is_some_and(|name| {
        name.eq_ignore_ascii_case("bin") || name.eq_ignore_ascii_case("Scripts")
    }) {
        directory
            .parent()
            .context("helper Python has no environment prefix")?
    } else {
        directory
    };
    // Resolve directory aliases without following a venv's Python symlink into its base install.
    prefix
        .canonicalize()
        .with_context(|| format!("cannot resolve helper prefix {}", prefix.display()))
}

pub(crate) fn discover(python: &str) -> Result<Map<String, Value>> {
    let prefix = helper_prefix(
        Path::new(python),
        &env::current_dir()?,
        env::var_os("PATH")
            .as_deref()
            .unwrap_or_else(|| OsStr::new("")),
    )?;
    let home = env_path(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .context("home directory is required for kernel discovery")?;
    let home = home.canonicalize().unwrap_or(home);
    let environment = prefix.join("share/jupyter");
    let platform_dirs = env_flag("JUPYTER_PLATFORM_DIRS").unwrap_or(false);
    let app =
        if cfg!(windows) || (cfg!(target_os = "macos") && !prefix.starts_with("/opt/homebrew")) {
            "Jupyter"
        } else {
            "jupyter"
        };
    let data = env_path("JUPYTER_DATA_DIR").unwrap_or_else(|| {
        if cfg!(windows) {
            env_path(if platform_dirs {
                "LOCALAPPDATA"
            } else {
                "APPDATA"
            })
            .map(|path| path.join(if platform_dirs { app } else { "jupyter" }))
            .unwrap_or_else(|| {
                env_path("JUPYTER_CONFIG_DIR")
                    .unwrap_or_else(|| home.join(".jupyter"))
                    .join("data")
            })
        } else if cfg!(target_os = "macos") {
            home.join(if platform_dirs {
                "Library/Application Support"
            } else {
                "Library"
            })
            .join(if platform_dirs { app } else { "Jupyter" })
        } else {
            env_path("XDG_DATA_HOME")
                .unwrap_or_else(|| home.join(".local/share"))
                .join("jupyter")
        }
    });
    let system: Vec<PathBuf> = if cfg!(windows) {
        vec![if env_flag("JUPYTER_USE_PROGRAMDATA").unwrap_or(false) {
            env_path("PROGRAMDATA")
                .map(|path| path.join(if platform_dirs { app } else { "jupyter" }))
                .unwrap_or_else(|| environment.clone())
        } else {
            environment.clone()
        }]
    } else if platform_dirs && cfg!(target_os = "macos") {
        vec![Path::new("/Library/Application Support").join(app)]
    } else if platform_dirs {
        env::split_paths(
            &env::var_os("XDG_DATA_DIRS")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
        )
        .map(|path| path.join("jupyter"))
        .collect()
    } else {
        vec![
            "/usr/local/share/jupyter".into(),
            "/usr/share/jupyter".into(),
        ]
    };
    let venv = fs::read_to_string(prefix.join("pyvenv.cfg")).ok();
    let mut user = vec![data];
    let user_site = !env::var_os("PYTHONNOUSERSITE").is_some_and(|value| !value.is_empty())
        && venv.as_ref().is_none_or(|config| {
            config.lines().any(|line| {
                line.split_once('=').is_some_and(|(key, value)| {
                    key.trim()
                        .eq_ignore_ascii_case("include-system-site-packages")
                        && value.trim().eq_ignore_ascii_case("true")
                })
            })
        });
    if user_site {
        let base = env_path("PYTHONUSERBASE").unwrap_or_else(|| {
            if cfg!(windows) {
                env_path("APPDATA")
                    .unwrap_or_else(|| home.clone())
                    .join("Python")
            } else {
                home.join(".local")
            }
        });
        let path = base.join("share/jupyter");
        if !user.contains(&path) {
            user.push(path);
        }
    }
    let conda = env_path("CONDA_PREFIX").is_some_and(|path| prefix.starts_with(path))
        && env::var("CONDA_DEFAULT_ENV").unwrap_or_else(|_| "base".into()) != "base";
    let prefer_env = env_flag("JUPYTER_PREFER_ENV_PATH")
        .unwrap_or_else(|| (venv.is_some() || conda) && owned(&prefix));
    let mut paths: Vec<PathBuf> = env::var_os("JUPYTER_PATH")
        .filter(|value| !value.is_empty())
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default();
    let environment = if system == [environment.clone()] || !system.contains(&environment) {
        vec![environment]
    } else {
        Vec::new()
    };
    if prefer_env {
        paths.extend(environment);
        paths.extend(user);
    } else {
        paths.extend(user);
        paths.extend(environment);
    }
    for path in system {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    let mut paths: Vec<_> = paths.into_iter().map(|path| path.join("kernels")).collect();
    let mut sites = vec![
        prefix.join("Lib/site-packages"),
        prefix.join("lib/python3/dist-packages"),
    ];
    for lib in ["lib", "lib64"] {
        if let Ok(entries) = fs::read_dir(prefix.join(lib)) {
            sites.extend(
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with("python3"))
                    .flat_map(|entry| {
                        [
                            entry.path().join("site-packages"),
                            entry.path().join("dist-packages"),
                        ]
                    }),
            );
        }
    }
    if sites.iter().any(|path| path.join("IPython").is_dir()) {
        // shortcut: legacy IPython temporary-dir and deprecated XDG migration are not reproduced.
        let ipython = env_path("IPYTHONDIR").unwrap_or_else(|| home.join(".ipython"));
        let ipython = ipython
            .strip_prefix("~")
            .map(|path| home.join(path))
            .unwrap_or(ipython);
        paths.push(ipython.join("kernels"));
    }
    // shortcut: custom Python import hooks and multi-version prefixes require explicit JUPYTER_PATH.
    let native = sites
        .iter()
        .map(|site| site.join("ipykernel/resources"))
        .find(|path| path.is_dir());
    discover_paths(&paths, native.as_deref())
}

fn discover_paths(paths: &[PathBuf], native: Option<&Path>) -> Result<Map<String, Value>> {
    let mut specs = Map::new();
    for directory in paths {
        if !directory.is_dir() {
            continue;
        }
        let mut local = Map::new();
        for entry in fs::read_dir(directory)
            .with_context(|| format!("cannot list kernels in {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() && path.join("kernel.json").is_file() {
                // Jupyter warns about invalid names, but includes them and hidden directories.
                local.insert(
                    entry.file_name().to_string_lossy().to_lowercase(),
                    Value::String(path.to_string_lossy().into_owned()),
                );
            }
        }
        for (name, path) in local {
            specs.entry(name).or_insert(path);
        }
    }
    if let Some(native) = native {
        specs
            .entry("python3")
            .or_insert_with(|| Value::String(native.to_string_lossy().into_owned()));
    }
    Ok(specs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_invalid_entries() -> Result<()> {
        let root = tempfile::tempdir()?;
        let bin = root.path().join("bin");
        fs::create_dir(&bin)?;
        let python = bin.join(if cfg!(windows) {
            "python3.exe"
        } else {
            "python3"
        });
        fs::write(&python, "")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&python, fs::Permissions::from_mode(0o700))?;
            fs::create_dir_all(root.path().join("venv/bin"))?;
            let link = root.path().join("venv/bin/python3");
            std::os::unix::fs::symlink(&python, &link)?;
            assert_eq!(
                helper_prefix(&link, root.path(), OsStr::new(""))?,
                root.path().join("venv").canonicalize()?
            );
        }
        let expected = root.path().canonicalize()?;
        assert_eq!(
            helper_prefix(Path::new("python3"), root.path(), OsStr::new("bin"))?,
            expected
        );
        assert_eq!(
            helper_prefix(
                &Path::new("bin").join(python.file_name().unwrap()),
                root.path(),
                OsStr::new("")
            )?,
            expected
        );
        assert_eq!(
            helper_prefix(&root.path().join("python.exe"), root.path(), OsStr::new(""))?,
            expected
        );
        let first = root.path().join("first");
        let second = root.path().join("second");
        for path in [
            first.join("Julia"),
            second.join("julia"),
            first.join(".hidden"),
            first.join("bad name"),
            first.join("broken-json"),
            first.join("missing-json"),
            first.join("directory-json/kernel.json"),
        ] {
            fs::create_dir_all(path)?;
        }
        for path in [
            first.join("Julia"),
            second.join("julia"),
            first.join(".hidden"),
            first.join("bad name"),
            first.join("broken-json"),
        ] {
            fs::write(path.join("kernel.json"), "not parsed during discovery")?;
        }
        fs::write(first.join("not-a-directory"), "")?;
        let native = root.path().join("resources");
        let specs = discover_paths(&[first.clone(), second.clone()], Some(&native))?;
        assert_eq!(
            specs["julia"],
            first.join("Julia").to_string_lossy().as_ref()
        );
        assert!(specs.contains_key(".hidden") && specs.contains_key("bad name"));
        assert!(specs.contains_key("broken-json"));
        assert!(!specs.contains_key("missing-json") && !specs.contains_key("directory-json"));
        assert!(!specs.contains_key("not-a-directory"));
        assert_eq!(specs["python3"], native.to_string_lossy().as_ref());
        fs::create_dir_all(second.join("Python3"))?;
        fs::write(second.join("Python3/kernel.json"), "{}")?;
        let specs = discover_paths(&[first, second.clone()], Some(&native))?;
        assert_eq!(
            specs["python3"],
            second.join("Python3").to_string_lossy().as_ref()
        );
        Ok(())
    }
}
