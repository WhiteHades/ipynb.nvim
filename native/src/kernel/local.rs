//! Kernelspec launch and connection-file storage.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use uuid::Uuid;

const NUMPY_STARTUP: &str = "exec('''try:\n    import numpy\nexcept ImportError:\n    pass\nelse:\n    if int(numpy.__version__.split('.')[0]) >= 2:\n        numpy.set_printoptions(legacy='1.25')\n''', {})";

#[derive(Clone, Debug)]
pub(super) struct ConnectionInfo {
    pub(super) shell_port: u16,
    pub(super) iopub_port: u16,
    pub(super) stdin_port: u16,
    pub(super) control_port: u16,
    pub(super) ip: String,
    pub(super) key: Vec<u8>,
    pub(super) transport: String,
    pub(super) signature_scheme: String,
}

#[derive(Clone, Debug)]
pub(super) struct KernelSpec {
    pub(super) argv: Vec<String>,
    pub(super) env: Vec<(String, String)>,
    language: String,
    resource_dir: PathBuf,
    name: String,
}

pub(super) struct LocalConfig {
    pub(super) spec: KernelSpec,
    pub(super) python: Option<String>,
    pub(super) numpy_legacy_repr: bool,
    pub(super) connection_file: PathBuf,
    pub(super) cwd: Option<PathBuf>,
}

impl LocalConfig {
    pub(super) fn language(&self) -> &str {
        &self.spec.language
    }
}

pub(super) struct LocalPrepared {
    pub(super) config: LocalConfig,
    pub(super) child: Child,
    pub(super) connection: ConnectionInfo,
}

pub(super) fn prepare_local(
    name: &str,
    python: Option<&str>,
    numpy_legacy_repr: bool,
    spec_path: &Path,
    cwd: Option<&Path>,
) -> Result<LocalPrepared> {
    let spec = read_kernel_spec(spec_path, name)
        .with_context(|| format!("read kernelspec {}", spec_path.display()))?;
    let connection_file = unique_connection_file()?;
    let config = LocalConfig {
        spec,
        python: python.map(str::to_string),
        numpy_legacy_repr,
        connection_file,
        cwd: cwd.map(Path::to_path_buf),
    };
    let (connection, child) = launch_local(&config)?;
    Ok(LocalPrepared {
        config,
        child,
        connection,
    })
}

pub(super) fn read_kernel_spec(path: &Path, fallback_name: &str) -> Result<KernelSpec> {
    let value: Value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("decode {}", path.display()))?;
    let argv = value
        .get("argv")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("kernelspec {} has no argv", path.display()))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("kernelspec argv contains a non-string value"))
        })
        .collect::<Result<Vec<_>>>()?;
    if argv.is_empty() {
        return Err(anyhow!("kernelspec {} has an empty argv", path.display()));
    }
    let env = match value.get("env") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(values)) => values
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .ok_or_else(|| anyhow!("kernelspec env value for {key} is not a string"))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(_) => return Err(anyhow!("kernelspec env is not an object")),
    };
    let language = value
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let name = fallback_name.to_string();
    let resource_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(KernelSpec {
        argv,
        env,
        language,
        resource_dir,
        name,
    })
}

fn unique_connection_file() -> Result<PathBuf> {
    let directory = std::env::temp_dir().join("ipynb.nvim");
    fs::create_dir_all(&directory)
        .with_context(|| format!("create connection directory {}", directory.display()))?;
    Ok(directory.join(format!("kernel-{}.json", Uuid::new_v4())))
}

pub(super) fn launch_local(config: &LocalConfig) -> Result<(ConnectionInfo, Child)> {
    let connection = new_connection(&config.spec.name)?;
    write_connection_file(&config.connection_file, &connection, &config.spec.name)?;

    let mut argv = config
        .spec
        .argv
        .iter()
        .map(|argument| {
            argument
                .replace(
                    "{connection_file}",
                    &config.connection_file.to_string_lossy(),
                )
                .replace(
                    "{resource_dir}",
                    &config.spec.resource_dir.to_string_lossy(),
                )
                .replace("{kernel_name}", &config.spec.name)
        })
        .collect::<Vec<_>>();

    if let Some(python) = &config.python
        && matches!(argv.first().map(String::as_str), Some("python" | "python3"))
    {
        argv[0] = python.clone();
    }
    if config.numpy_legacy_repr && config.spec.language.eq_ignore_ascii_case("python") {
        let exec_lines = serde_json::to_string(&vec![NUMPY_STARTUP])?;
        argv.push(format!("--IPKernelApp.exec_lines={exec_lines}"));
    }

    let mut command = Command::new(&argv[0]);
    if let Some(cwd) = &config.cwd {
        command.current_dir(cwd);
    }
    command
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let executable = Path::new(&argv[0]);
    if executable.is_absolute()
        && let Some(bin) = executable.parent()
        && let Some(venv) = bin
            .parent()
            .filter(|venv| venv.join("pyvenv.cfg").is_file())
    {
        let mut paths = vec![bin.to_owned()];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        command.env("VIRTUAL_ENV", venv).env(
            "PATH",
            std::env::join_paths(paths).context("build kernel virtualenv PATH")?,
        );
    }
    for (key, value) in &config.spec.env {
        command.env(key, expand_kernel_env(value));
    }
    let child = command
        .spawn()
        .with_context(|| format!("launch Jupyter kernel with {}", argv.join(" ")))?;
    Ok((connection, child))
}

fn expand_kernel_env(value: &str) -> String {
    fn is_name_start(byte: u8) -> bool {
        byte == b'_' || byte.is_ascii_alphabetic()
    }

    fn is_name_byte(byte: u8) -> bool {
        is_name_start(byte) || byte.is_ascii_digit()
    }

    let bytes = value.as_bytes();
    let mut expanded = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            let character = value[index..]
                .chars()
                .next()
                .expect("valid UTF-8 boundary while expanding kernel env");
            expanded.push(character);
            index += character.len_utf8();
            continue;
        }

        if index + 1 < bytes.len() && bytes[index + 1] == b'$' {
            expanded.push('$');
            index += 2;
            continue;
        }

        let (name_start, name_end, end) = if index + 2 < bytes.len()
            && bytes[index + 1] == b'{'
            && is_name_start(bytes[index + 2])
        {
            let name_start = index + 2;
            let mut name_end = name_start + 1;
            while name_end < bytes.len() && is_name_byte(bytes[name_end]) {
                name_end += 1;
            }
            if name_end < bytes.len() && bytes[name_end] == b'}' {
                (name_start, name_end, name_end + 1)
            } else {
                expanded.push('$');
                index += 1;
                continue;
            }
        } else if index + 1 < bytes.len() && is_name_start(bytes[index + 1]) {
            let name_start = index + 1;
            let mut name_end = name_start + 1;
            while name_end < bytes.len() && is_name_byte(bytes[name_end]) {
                name_end += 1;
            }
            (name_start, name_end, name_end)
        } else {
            expanded.push('$');
            index += 1;
            continue;
        };

        let name = &value[name_start..name_end];
        match std::env::var(name) {
            Ok(replacement) => expanded.push_str(&replacement),
            Err(_) => expanded.push_str(&value[index..end]),
        }
        index = end;
    }
    expanded
}

fn new_connection(_kernel_name: &str) -> Result<ConnectionInfo> {
    Ok(ConnectionInfo {
        shell_port: free_port()?,
        iopub_port: free_port()?,
        stdin_port: free_port()?,
        control_port: free_port()?,
        ip: "127.0.0.1".to_string(),
        key: Uuid::new_v4().to_string().into_bytes(),
        transport: "tcp".to_string(),
        signature_scheme: "hmac-sha256".to_string(),
    })
}

pub(super) fn free_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("allocate Jupyter port")?;
    Ok(listener.local_addr()?.port())
}

pub(super) fn write_connection_file(
    path: &Path,
    connection: &ConnectionInfo,
    kernel_name: &str,
) -> Result<()> {
    let value = json!({
        "shell_port": connection.shell_port,
        "iopub_port": connection.iopub_port,
        "stdin_port": connection.stdin_port,
        "control_port": connection.control_port,
        "hb_port": 0,
        "ip": connection.ip,
        "key": String::from_utf8_lossy(&connection.key),
        "transport": connection.transport,
        "signature_scheme": connection.signature_scheme,
        "kernel_name": kernel_name,
    });
    fs::write(path, serde_json::to_vec_pretty(&value)?)
        .with_context(|| format!("write {}", path.display()))?;
    set_private_file(path)?;
    Ok(())
}

pub(super) fn read_connection_file(path: &Path) -> Result<ConnectionInfo> {
    let value: Value = serde_json::from_slice(&fs::read(path)?)?;
    connection_from_value(&value)
}

fn connection_from_value(value: &Value) -> Result<ConnectionInfo> {
    Ok(ConnectionInfo {
        shell_port: json_port(value, "shell_port")?,
        iopub_port: json_port(value, "iopub_port")?,
        stdin_port: json_port(value, "stdin_port")?,
        control_port: json_port(value, "control_port")?,
        ip: value
            .get("ip")
            .and_then(Value::as_str)
            .unwrap_or("127.0.0.1")
            .to_string(),
        key: value
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .as_bytes()
            .to_vec(),
        transport: value
            .get("transport")
            .and_then(Value::as_str)
            .unwrap_or("tcp")
            .to_string(),
        signature_scheme: value
            .get("signature_scheme")
            .and_then(Value::as_str)
            .unwrap_or("hmac-sha256")
            .to_string(),
    })
}

fn json_port(value: &Value, key: &str) -> Result<u16> {
    let number = value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("connection file has no valid {key}"))?;
    u16::try_from(number).with_context(|| format!("invalid {key} port {number}"))
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}
