//! Synchronous Jupyter kernel transport.
//!
//! Every transport socket belongs to one worker thread.  Callers submit
//! commands and drain ordinary JSON messages, leaving UI and Neovim concerns
//! to the adapter layer.

use std::collections::VecDeque;
use std::fs;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use chrono::{SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

const MESSAGE_DELIMITER: &[u8] = b"<IDS|MSG>";
const PROTOCOL_VERSION: &str = "5.3";
const NUMPY_STARTUP: &str = "exec('''try:\n    import numpy\nexcept ImportError:\n    pass\nelse:\n    if int(numpy.__version__.split('.')[0]) >= 2:\n        numpy.set_printoptions(legacy='1.25')\n''', {})";

/// A running Jupyter kernel.
pub struct Kernel {
    commands: Sender<CommandMessage>,
    events: Receiver<Value>,
    worker: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
    restartable: bool,
    language: Option<String>,
}

impl Kernel {
    /// Start a local kernelspec, attach to a connection file, or create a
    /// kernel through a Jupyter Server URL.
    pub fn start(name: &str, python: Option<&str>, numpy_legacy_repr: bool) -> Result<Self> {
        let prepared = prepare_kernel(name, python, numpy_legacy_repr)?;
        let restartable = prepared.restartable();
        let language = prepared.language();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("ipynb-jupyter-kernel".to_string())
            .spawn(move || worker_main(prepared, command_rx, event_tx))
            .context("spawn Jupyter kernel worker")?;

        Ok(Self {
            commands: command_tx,
            events: event_rx,
            worker: Mutex::new(Some(worker)),
            closed: AtomicBool::new(false),
            restartable,
            language,
        })
    }

    /// Return the kernelspec language when it is known before the first
    /// kernel_info_reply. Server and connection-file transports report the
    /// authoritative language through that reply in drain().
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Queue code for execution and return its Jupyter message id.
    pub fn execute(&self, code: &str) -> Result<String> {
        self.ensure_open()?;
        let msg_id = Uuid::new_v4().to_string();
        self.commands
            .send(CommandMessage::Execute {
                msg_id: msg_id.clone(),
                code: code.to_string(),
            })
            .context("send execute request to kernel worker")?;
        Ok(msg_id)
    }

    /// Reply to the most recent input request.
    pub fn stdin(&self, value: &str) -> Result<()> {
        self.ensure_open()?;
        self.commands
            .send(CommandMessage::Input(value.to_string()))
            .context("send stdin reply to kernel worker")?;
        Ok(())
    }

    /// Interrupt the currently running execution.
    pub fn interrupt(&self) -> Result<()> {
        self.ensure_open()?;
        self.commands
            .send(CommandMessage::Interrupt)
            .context("send interrupt request to kernel worker")?;
        Ok(())
    }

    /// Restart an owned local or Jupyter Server kernel.
    pub fn restart(&self) -> Result<()> {
        self.ensure_open()?;
        if !self.restartable {
            return Err(anyhow!(
                "cannot restart a kernel opened from an external connection file"
            ));
        }
        self.commands
            .send(CommandMessage::Restart)
            .context("send restart request to kernel worker")?;
        Ok(())
    }

    /// Stop the worker and release owned process/file resources.
    pub fn shutdown(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.commands
            .send(CommandMessage::Shutdown)
            .context("send shutdown request to kernel worker")?;
        self.join_worker()
    }

    /// Return every message currently available without waiting.
    pub fn drain(&self) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(message) => messages.push(message),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        messages
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            Err(anyhow!("Jupyter kernel is shut down"))
        } else {
            Ok(())
        }
    }

    fn join_worker(&self) -> Result<()> {
        let worker = self
            .worker
            .lock()
            .map_err(|_| anyhow!("kernel worker lock poisoned"))?
            .take();
        if let Some(worker) = worker {
            worker
                .join()
                .map_err(|_| anyhow!("Jupyter kernel worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for Kernel {
    fn drop(&mut self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            let _ = self.commands.send(CommandMessage::Shutdown);
        }
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
    }
}

enum CommandMessage {
    Execute { msg_id: String, code: String },
    Input(String),
    Interrupt,
    Restart,
    Shutdown,
}

#[derive(Clone, Debug)]
struct ConnectionInfo {
    shell_port: u16,
    iopub_port: u16,
    stdin_port: u16,
    control_port: u16,
    ip: String,
    key: Vec<u8>,
    transport: String,
    signature_scheme: String,
}

#[derive(Clone, Debug)]
struct KernelSpec {
    argv: Vec<String>,
    env: Vec<(String, String)>,
    language: String,
    resource_dir: PathBuf,
    name: String,
}

struct LocalConfig {
    spec: KernelSpec,
    python: Option<String>,
    numpy_legacy_repr: bool,
    connection_file: PathBuf,
}

struct LocalPrepared {
    config: LocalConfig,
    child: Child,
    connection: ConnectionInfo,
}

struct ExternalPrepared {
    config: ExternalConfig,
    kernel_id: String,
    socket: ExternalSocket,
}

enum PreparedKernel {
    Local(LocalPrepared),
    Attached { connection: ConnectionInfo },
    External(ExternalPrepared),
}

impl PreparedKernel {
    fn restartable(&self) -> bool {
        !matches!(self, Self::Attached { .. })
    }

    fn language(&self) -> Option<String> {
        match self {
            Self::Local(local) => Some(local.config.spec.language.clone()),
            Self::Attached { .. } | Self::External(_) => None,
        }
    }
}

fn prepare_kernel(
    name: &str,
    python: Option<&str>,
    numpy_legacy_repr: bool,
) -> Result<PreparedKernel> {
    if name.starts_with("http://") || name.starts_with("https://") {
        return prepare_external(name);
    }

    let path = Path::new(name);
    if path.exists() || name.ends_with(".json") {
        return Ok(PreparedKernel::Attached {
            connection: read_connection_file(path)
                .with_context(|| format!("read connection file {}", path.display()))?,
        });
    }

    prepare_local(name, python, numpy_legacy_repr).map(PreparedKernel::Local)
}

fn prepare_local(
    name: &str,
    python: Option<&str>,
    numpy_legacy_repr: bool,
) -> Result<LocalPrepared> {
    let spec_path = find_kernel_spec(name, python)?;
    let spec = read_kernel_spec(&spec_path, name)
        .with_context(|| format!("read kernelspec {}", spec_path.display()))?;
    let connection_file = unique_connection_file()?;
    let config = LocalConfig {
        spec,
        python: python.map(str::to_string),
        numpy_legacy_repr,
        connection_file,
    };
    let (connection, child) = launch_local(&config)?;
    Ok(LocalPrepared {
        config,
        child,
        connection,
    })
}

/// List kernelspec keys using the selected Python environment.
pub fn available_kernels(python: Option<&str>) -> Result<Vec<String>> {
    let executable = python.unwrap_or("python3");
    let output = Command::new(executable)
        .args(["-m", "jupyter", "kernelspec", "list", "--json"])
        .output()
        .with_context(|| format!("discover kernelspecs with {executable}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "jupyter kernelspec discovery failed with {}",
            output.status
        ));
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).context("decode jupyter kernelspec list output")?;
    let mut names = value
        .get("kernelspecs")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("jupyter kernelspec list has no kernelspecs object"))?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

fn find_kernel_spec(name: &str, python: Option<&str>) -> Result<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(value) = std::env::var("JUPYTER_PATH") {
        roots.extend(std::env::split_paths(&value));
    }
    if let Ok(value) = std::env::var("JUPYTER_DATA_DIR") {
        roots.push(PathBuf::from(value));
    }
    if let Ok(value) = std::env::var("XDG_DATA_HOME") {
        roots.push(PathBuf::from(value).join("jupyter"));
    }
    if let Some(home) = home_dir() {
        roots.push(home.join(".local/share/jupyter"));
        roots.push(home.join(".jupyter"));
    }
    if let Some(python) = python {
        if let Some(prefix) = Path::new(python).parent().and_then(Path::parent) {
            roots.push(prefix.join("share/jupyter"));
        }
    }
    roots.push(PathBuf::from("/usr/local/share/jupyter"));
    roots.push(PathBuf::from("/usr/share/jupyter"));

    for root in roots {
        let candidate = root.join("kernels").join(name).join("kernel.json");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let executable = python.unwrap_or("python3");
    let output = Command::new(executable)
        .args(["-m", "jupyter", "kernelspec", "list", "--json"])
        .output()
        .with_context(|| format!("discover kernelspecs with {executable}"))?;
    if output.status.success() {
        let value: Value = serde_json::from_slice(&output.stdout)
            .context("decode jupyter kernelspec list output")?;
        if let Some(path) = value
            .get("kernelspecs")
            .and_then(Value::as_object)
            .and_then(|specs| specs.get(name))
            .and_then(|spec| spec.get("resource_dir"))
            .and_then(Value::as_str)
        {
            let candidate = Path::new(path).join("kernel.json");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(anyhow!("Jupyter kernelspec not found: {name}"))
}

fn read_kernel_spec(path: &Path, fallback_name: &str) -> Result<KernelSpec> {
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

fn launch_local(config: &LocalConfig) -> Result<(ConnectionInfo, Child)> {
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
    command
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("allocate Jupyter port")?;
    Ok(listener.local_addr()?.port())
}

fn write_connection_file(
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

fn read_connection_file(path: &Path) -> Result<ConnectionInfo> {
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

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

struct ZmqTransport {
    _context: zmq::Context,
    shell: zmq::Socket,
    iopub: zmq::Socket,
    stdin: zmq::Socket,
    control: zmq::Socket,
    connection: ConnectionInfo,
}

impl ZmqTransport {
    fn connect(connection: ConnectionInfo) -> Result<Self> {
        if connection.transport != "tcp" {
            return Err(anyhow!(
                "unsupported Jupyter transport: {}",
                connection.transport
            ));
        }
        if connection.signature_scheme != "hmac-sha256" {
            return Err(anyhow!(
                "unsupported Jupyter signature scheme: {}",
                connection.signature_scheme
            ));
        }
        let context = zmq::Context::new();
        let identity = Uuid::new_v4().to_string();
        let shell = connect_socket(
            &context,
            zmq::DEALER,
            &connection,
            connection.shell_port,
            identity.as_bytes(),
        )?;
        let iopub = connect_iopub(&context, &connection, connection.iopub_port)?;
        let stdin = connect_socket(
            &context,
            zmq::DEALER,
            &connection,
            connection.stdin_port,
            identity.as_bytes(),
        )?;
        let control = connect_socket(
            &context,
            zmq::DEALER,
            &connection,
            connection.control_port,
            identity.as_bytes(),
        )?;
        Ok(Self {
            _context: context,
            shell,
            iopub,
            stdin,
            control,
            connection,
        })
    }

    fn send_kernel_info(&self, session: &str) -> Result<()> {
        send_request(
            &self.shell,
            &self.connection,
            session,
            "kernel_info_request",
            json!({}),
            &json!({}),
        )?;
        Ok(())
    }

    fn execute(&self, session: &str, msg_id: &str, code: &str) -> Result<()> {
        send_request_with_id(
            &self.shell,
            &self.connection,
            session,
            msg_id,
            "execute_request",
            json!({
                "code": code,
                "silent": false,
                "store_history": true,
                "user_expressions": {},
                "allow_stdin": true,
                "stop_on_error": true,
            }),
            &json!({}),
        )
    }

    fn input(&self, session: &str, parent: &Value, value: &str) -> Result<()> {
        send_request(
            &self.stdin,
            &self.connection,
            session,
            "input_reply",
            json!({ "value": value }),
            parent,
        )?;
        Ok(())
    }

    fn interrupt(&self, session: &str) -> Result<()> {
        send_request(
            &self.control,
            &self.connection,
            session,
            "interrupt_request",
            json!({}),
            &json!({}),
        )?;
        Ok(())
    }

    fn shutdown(&self, session: &str, restart: bool) -> Result<()> {
        send_request(
            &self.control,
            &self.connection,
            session,
            "shutdown_request",
            json!({ "restart": restart }),
            &json!({}),
        )?;
        Ok(())
    }
}

fn connect_socket(
    context: &zmq::Context,
    socket_type: zmq::SocketType,
    connection: &ConnectionInfo,
    port: u16,
    identity: &[u8],
) -> Result<zmq::Socket> {
    let socket = context.socket(socket_type)?;
    socket.set_linger(0)?;
    socket.set_identity(identity)?;
    socket.connect(&endpoint(connection, port))?;
    Ok(socket)
}

fn connect_iopub(
    context: &zmq::Context,
    connection: &ConnectionInfo,
    port: u16,
) -> Result<zmq::Socket> {
    let socket = context.socket(zmq::SUB)?;
    socket.set_linger(0)?;
    socket.set_subscribe(b"")?;
    socket.connect(&endpoint(connection, port))?;
    Ok(socket)
}

fn endpoint(connection: &ConnectionInfo, port: u16) -> String {
    let ip = if connection.ip == "0.0.0.0" || connection.ip == "::" {
        "127.0.0.1"
    } else {
        connection.ip.as_str()
    };
    format!("{}://{}:{}", connection.transport, ip, port)
}

fn send_request(
    socket: &zmq::Socket,
    connection: &ConnectionInfo,
    session: &str,
    msg_type: &str,
    content: Value,
    parent_header: &Value,
) -> Result<String> {
    let msg_id = Uuid::new_v4().to_string();
    send_request_with_id(
        socket,
        connection,
        session,
        &msg_id,
        msg_type,
        content,
        parent_header,
    )?;
    Ok(msg_id)
}

fn send_request_with_id(
    socket: &zmq::Socket,
    connection: &ConnectionInfo,
    session: &str,
    msg_id: &str,
    msg_type: &str,
    content: Value,
    parent_header: &Value,
) -> Result<()> {
    let header = request_header(session, msg_id, msg_type);
    let metadata = json!({});
    let parts = [
        serde_json::to_vec(&header)?,
        serde_json::to_vec(parent_header)?,
        serde_json::to_vec(&metadata)?,
        serde_json::to_vec(&content)?,
    ];
    let signature = sign_parts(&connection.key, &parts);
    let mut frames: Vec<Vec<u8>> = vec![MESSAGE_DELIMITER.to_vec(), signature.into_bytes()];
    frames.extend(parts);
    socket.send_multipart(frames, 0)?;
    Ok(())
}

fn request_header(session: &str, msg_id: &str, msg_type: &str) -> Value {
    json!({
        "msg_id": msg_id,
        "username": "ipynb.nvim",
        "session": session,
        "date": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "msg_type": msg_type,
        "version": PROTOCOL_VERSION,
    })
}

fn sign_parts(key: &[u8], parts: &[Vec<u8>; 4]) -> String {
    if key.is_empty() {
        return String::new();
    }
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts every key length");
    for part in parts {
        mac.update(part);
    }
    let bytes = mac.finalize().into_bytes();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn receive_message(
    frames: &[Vec<u8>],
    connection: &ConnectionInfo,
    channel: &str,
) -> Result<Value> {
    let delimiter = frames
        .iter()
        .position(|frame| frame.as_slice() == MESSAGE_DELIMITER)
        .ok_or_else(|| anyhow!("Jupyter message has no message delimiter"))?;
    if frames.len() < delimiter + 6 {
        return Err(anyhow!("Jupyter message has incomplete JSON frames"));
    }
    let signature = std::str::from_utf8(&frames[delimiter + 1])?;
    let parts = [
        frames[delimiter + 2].clone(),
        frames[delimiter + 3].clone(),
        frames[delimiter + 4].clone(),
        frames[delimiter + 5].clone(),
    ];
    let expected = sign_parts(&connection.key, &parts);
    if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
        return Err(anyhow!("Jupyter message signature verification failed"));
    }
    let header: Value = serde_json::from_slice(&parts[0]).context("decode Jupyter header")?;
    let parent_header: Value = serde_json::from_slice(&parts[1]).context("decode parent header")?;
    let metadata: Value = serde_json::from_slice(&parts[2]).context("decode Jupyter metadata")?;
    let content: Value = serde_json::from_slice(&parts[3]).context("decode Jupyter content")?;
    let mut message = message_value(channel, header, parent_header, metadata, content);
    if frames.len() > delimiter + 6 {
        message["buffers"] = Value::Array(
            frames[delimiter + 6..]
                .iter()
                .map(|buffer| {
                    Value::String(base64::engine::general_purpose::STANDARD.encode(buffer))
                })
                .collect(),
        );
    }
    Ok(message)
}

fn message_value(
    channel: &str,
    header: Value,
    parent_header: Value,
    metadata: Value,
    content: Value,
) -> Value {
    let msg_type = header.get("msg_type").cloned().unwrap_or(Value::Null);
    json!({
        "channel": channel,
        "msg_type": msg_type,
        "header": header,
        "parent_header": parent_header,
        "metadata": metadata,
        "content": content,
    })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

struct SessionState {
    session_id: String,
    ready: bool,
    saw_kernel_info: bool,
    saw_idle: bool,
    pending: VecDeque<(String, String)>,
    input_parent: Value,
}

impl SessionState {
    fn new() -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            ready: false,
            saw_kernel_info: false,
            saw_idle: false,
            pending: VecDeque::new(),
            input_parent: json!({}),
        }
    }

    fn reset(&mut self) {
        self.ready = false;
        self.saw_kernel_info = false;
        self.saw_idle = false;
        self.pending.clear();
        self.input_parent = json!({});
    }

    fn observe(&mut self, message: &Value) -> bool {
        let msg_type = message
            .get("header")
            .and_then(|header| header.get("msg_type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if msg_type == "kernel_info_reply" {
            self.saw_kernel_info = true;
        }
        if msg_type == "status"
            && message
                .get("content")
                .and_then(|content| content.get("execution_state"))
                .and_then(Value::as_str)
                == Some("idle")
        {
            self.saw_idle = true;
        }
        if msg_type == "input_request" {
            self.input_parent = message.get("header").cloned().unwrap_or_else(|| json!({}));
        }
        if !self.ready && self.saw_kernel_info && self.saw_idle {
            self.ready = true;
            return true;
        }
        false
    }
}

struct ZmqWorker {
    transport: ZmqTransport,
    child: Option<Child>,
    local_config: Option<LocalConfig>,
}

impl ZmqWorker {
    fn from_prepared(prepared: PreparedKernel) -> Result<Self> {
        match prepared {
            PreparedKernel::Local(local) => {
                let LocalPrepared {
                    config,
                    mut child,
                    connection,
                } = local;
                let transport = match ZmqTransport::connect(connection) {
                    Ok(transport) => transport,
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = fs::remove_file(&config.connection_file);
                        return Err(error);
                    }
                };
                Ok(Self {
                    transport,
                    child: Some(child),
                    local_config: Some(config),
                })
            }
            PreparedKernel::Attached { connection } => Ok(Self {
                transport: ZmqTransport::connect(connection)?,
                child: None,
                local_config: None,
            }),
            PreparedKernel::External(_) => Err(anyhow!("external kernel is not ZeroMQ")),
        }
    }

    fn execute(&self, session: &str, msg_id: &str, code: &str) -> Result<()> {
        self.transport.execute(session, msg_id, code)
    }

    fn input(&self, session: &str, parent: &Value, value: &str) -> Result<()> {
        self.transport.input(session, parent, value)
    }

    fn interrupt(&self, session: &str) -> Result<()> {
        self.transport.interrupt(session)
    }

    fn restart(&mut self, session: &str) -> Result<()> {
        let config = self
            .local_config
            .as_ref()
            .ok_or_else(|| anyhow!("cannot restart an attached connection-file kernel"))?;
        self.transport.shutdown(session, false).ok();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let (connection, child) = launch_local(config)?;
        self.transport = ZmqTransport::connect(connection)?;
        self.child = Some(child);
        self.transport.send_kernel_info(session)
    }

    fn shutdown(&mut self, session: &str) {
        if self.child.is_some() {
            let _ = self.transport.shutdown(session, false);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(config) = self.local_config.take() {
            let _ = fs::remove_file(config.connection_file);
        }
    }
}

fn worker_main(
    prepared: PreparedKernel,
    commands: Receiver<CommandMessage>,
    events: Sender<Value>,
) {
    match prepared {
        PreparedKernel::External(external) => run_external_worker(external, commands, events),
        prepared => match ZmqWorker::from_prepared(prepared) {
            Ok(worker) => run_zmq_worker(worker, commands, events),
            Err(error) => emit_error(&events, "startup", error),
        },
    }
}

fn run_zmq_worker(
    mut worker: ZmqWorker,
    commands: Receiver<CommandMessage>,
    events: Sender<Value>,
) {
    let mut session = SessionState::new();
    if let Err(error) = worker.transport.send_kernel_info(&session.session_id) {
        emit_error(&events, "shell", error);
        worker.shutdown(&session.session_id);
        return;
    }

    'worker: loop {
        loop {
            match commands.try_recv() {
                Ok(command) => {
                    if handle_zmq_command(command, &mut worker, &mut session, &events) {
                        break 'worker;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'worker,
            }
        }

        let mut poll_items = [
            worker.transport.iopub.as_poll_item(zmq::POLLIN),
            worker.transport.shell.as_poll_item(zmq::POLLIN),
            worker.transport.stdin.as_poll_item(zmq::POLLIN),
            worker.transport.control.as_poll_item(zmq::POLLIN),
        ];
        if let Err(error) = zmq::poll(&mut poll_items, 20) {
            emit_error(&events, "transport", error);
            break;
        }
        let sockets = [
            (&worker.transport.iopub, "iopub"),
            (&worker.transport.shell, "shell"),
            (&worker.transport.stdin, "stdin"),
            (&worker.transport.control, "control"),
        ];
        for (index, (socket, channel)) in sockets.iter().enumerate() {
            if !poll_items[index].get_revents().contains(zmq::POLLIN) {
                continue;
            }
            let frames = match socket.recv_multipart(0) {
                Ok(frames) => frames,
                Err(error) => {
                    emit_error(&events, channel, error);
                    continue;
                }
            };
            match receive_message(&frames, &worker.transport.connection, channel) {
                Ok(message) => {
                    let became_ready = session.observe(&message);
                    if events.send(message).is_err() {
                        break 'worker;
                    }
                    if became_ready {
                        if events.send(ready_message()).is_err() {
                            break 'worker;
                        }
                        while let Some((msg_id, code)) = session.pending.pop_front() {
                            if let Err(error) = worker.execute(&session.session_id, &msg_id, &code)
                            {
                                emit_error(&events, "shell", error);
                                break;
                            }
                        }
                    }
                }
                Err(error) => emit_error(&events, channel, error),
            }
        }
    }
    worker.shutdown(&session.session_id);
}

fn handle_zmq_command(
    command: CommandMessage,
    worker: &mut ZmqWorker,
    session: &mut SessionState,
    events: &Sender<Value>,
) -> bool {
    match command {
        CommandMessage::Execute { msg_id, code } => {
            if session.ready {
                if let Err(error) = worker.execute(&session.session_id, &msg_id, &code) {
                    emit_error(events, "shell", error);
                }
            } else {
                session.pending.push_back((msg_id, code));
            }
            false
        }
        CommandMessage::Input(value) => {
            if let Err(error) = worker.input(&session.session_id, &session.input_parent, &value) {
                emit_error(events, "stdin", error);
            }
            false
        }
        CommandMessage::Interrupt => {
            if let Err(error) = worker.interrupt(&session.session_id) {
                emit_error(events, "control", error);
            }
            false
        }
        CommandMessage::Restart => {
            session.reset();
            if let Err(error) = worker.restart(&session.session_id) {
                emit_error(events, "restart", error);
            }
            false
        }
        CommandMessage::Shutdown => true,
    }
}

fn ready_message() -> Value {
    json!({
        "channel": "runtime",
        "msg_type": "ready",
        "header": { "msg_type": "ready" },
        "parent_header": {},
        "metadata": {},
        "content": {},
    })
}

fn emit_error(events: &Sender<Value>, channel: &str, error: impl std::fmt::Display) {
    let _ = events.send(json!({
        "channel": channel,
        "msg_type": "transport_error",
        "header": { "msg_type": "transport_error" },
        "parent_header": {},
        "metadata": {},
        "content": { "error": error.to_string() },
    }));
}

use reqwest::Url;
use reqwest::blocking::Client;
use tungstenite::http::Request;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket, connect};

type ExternalSocket = WebSocket<MaybeTlsStream<TcpStream>>;

struct ExternalConfig {
    client: Client,
    base_url: Url,
    token: Option<String>,
}

struct ExternalWorker {
    config: ExternalConfig,
    kernel_id: String,
    socket: ExternalSocket,
}

fn prepare_external(name: &str) -> Result<PreparedKernel> {
    let mut base_url = Url::parse(name).context("parse Jupyter Server URL")?;
    let token = base_url
        .query_pairs()
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned());
    base_url.set_query(None);
    base_url.set_fragment(None);

    let client = Client::builder()
        .build()
        .context("create Jupyter HTTP client")?;
    let mut request = client.post(join_url(&base_url, "api/kernels"));
    if let Some(token) = &token {
        request = request.header("Authorization", format!("token {token}"));
    }
    let response = request
        .send()
        .context("create Jupyter Server kernel")?
        .error_for_status()
        .context("Jupyter Server rejected kernel creation")?;
    let kernel_info: Value = response.json().context("decode Jupyter Server response")?;
    let kernel_id = kernel_info
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Jupyter Server response has no kernel id"))?
        .to_string();

    let mut websocket_url = base_url.clone();
    websocket_url
        .set_scheme(if base_url.scheme() == "https" {
            "wss"
        } else {
            "ws"
        })
        .map_err(|_| anyhow!("invalid websocket URL scheme"))?;
    let path = format!(
        "{}/api/kernels/{kernel_id}/channels",
        base_url.path().trim_end_matches('/')
    );
    websocket_url.set_path(&path);
    let websocket_request = if let Some(token) = &token {
        Request::builder()
            .uri(websocket_url.as_str())
            .header("Authorization", format!("token {token}"))
            .body(())?
    } else {
        Request::builder().uri(websocket_url.as_str()).body(())?
    };
    let (socket, _) = connect(websocket_request).context("connect Jupyter Server websocket")?;
    let socket = set_websocket_timeout(socket);
    Ok(PreparedKernel::External(ExternalPrepared {
        config: ExternalConfig {
            client,
            base_url,
            token,
        },
        kernel_id,
        socket,
    }))
}

fn join_url(base: &Url, suffix: &str) -> String {
    format!(
        "{}/{}",
        base.as_str().trim_end_matches('/'),
        suffix.trim_start_matches('/')
    )
}

fn set_websocket_timeout(mut socket: ExternalSocket) -> ExternalSocket {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        }
        MaybeTlsStream::Rustls(stream) => {
            let _ = stream
                .sock
                .set_read_timeout(Some(Duration::from_millis(20)));
            let _ = stream.sock.set_write_timeout(Some(Duration::from_secs(5)));
        }
        _ => {}
    }
    socket
}

impl ExternalWorker {
    fn execute(&mut self, session: &str, msg_id: &str, code: &str) -> Result<()> {
        let message = request_value(
            session,
            msg_id,
            "execute_request",
            json!({
                "code": code,
                "silent": false,
                "store_history": true,
                "user_expressions": {},
                "allow_stdin": true,
                "stop_on_error": true,
            }),
            &json!({}),
        );
        self.socket
            .send(Message::Text(message.to_string().into()))?;
        Ok(())
    }

    fn input(&mut self, session: &str, parent: &Value, value: &str) -> Result<()> {
        let message = request_value(
            session,
            &Uuid::new_v4().to_string(),
            "input_reply",
            json!({ "value": value }),
            parent,
        );
        self.socket
            .send(Message::Text(message.to_string().into()))?;
        Ok(())
    }

    fn interrupt(&self) -> Result<()> {
        let request = self.authorized(
            self.config
                .client
                .post(format!("{}/interrupt", self.kernel_api_url())),
        );
        request.send()?.error_for_status()?;
        Ok(())
    }

    fn restart(&self) -> Result<()> {
        let request = self.authorized(
            self.config
                .client
                .post(format!("{}/restart", self.kernel_api_url())),
        );
        request.send()?.error_for_status()?;
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        let request = self.authorized(self.config.client.delete(self.kernel_api_url()));
        request.send()?.error_for_status()?;
        Ok(())
    }

    fn kernel_api_url(&self) -> String {
        format!(
            "{}/api/kernels/{}",
            self.config.base_url.as_str().trim_end_matches('/'),
            self.kernel_id
        )
    }

    fn authorized(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        if let Some(token) = &self.config.token {
            request.header("Authorization", format!("token {token}"))
        } else {
            request
        }
    }

    fn send_kernel_info(&mut self, session: &str) -> Result<()> {
        let message = request_value(
            session,
            &Uuid::new_v4().to_string(),
            "kernel_info_request",
            json!({}),
            &json!({}),
        );
        self.socket
            .send(Message::Text(message.to_string().into()))?;
        Ok(())
    }
}

fn run_external_worker(
    prepared: ExternalPrepared,
    commands: Receiver<CommandMessage>,
    events: Sender<Value>,
) {
    let mut worker = ExternalWorker {
        config: prepared.config,
        kernel_id: prepared.kernel_id,
        socket: prepared.socket,
    };
    let mut session = SessionState::new();
    if let Err(error) = worker.send_kernel_info(&session.session_id) {
        emit_error(&events, "websocket", error);
        return;
    }

    'worker: loop {
        loop {
            match commands.try_recv() {
                Ok(command) => {
                    if handle_external_command(command, &mut worker, &mut session, &events) {
                        break 'worker;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'worker,
            }
        }

        match worker.socket.read() {
            Ok(Message::Text(text)) => {
                if !handle_external_message(text.as_ref(), &mut worker, &mut session, &events) {
                    break 'worker;
                }
            }
            Ok(Message::Binary(bytes)) => {
                let text = match std::str::from_utf8(&bytes) {
                    Ok(text) => text,
                    Err(error) => {
                        emit_error(&events, "websocket", error);
                        continue;
                    }
                };
                if !handle_external_message(text, &mut worker, &mut session, &events) {
                    break 'worker;
                }
            }
            Ok(Message::Ping(payload)) => {
                if worker.socket.send(Message::Pong(payload)).is_err() {
                    break 'worker;
                }
            }
            Ok(Message::Close(_)) => break 'worker,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => {
                emit_error(&events, "websocket", error);
                break 'worker;
            }
        }
    }
    let _ = worker.shutdown();
}

fn handle_external_command(
    command: CommandMessage,
    worker: &mut ExternalWorker,
    session: &mut SessionState,
    events: &Sender<Value>,
) -> bool {
    match command {
        CommandMessage::Execute { msg_id, code } => {
            if session.ready {
                if let Err(error) = worker.execute(&session.session_id, &msg_id, &code) {
                    emit_error(events, "websocket", error);
                }
            } else {
                session.pending.push_back((msg_id, code));
            }
            false
        }
        CommandMessage::Input(value) => {
            if let Err(error) = worker.input(&session.session_id, &session.input_parent, &value) {
                emit_error(events, "websocket", error);
            }
            false
        }
        CommandMessage::Interrupt => {
            if let Err(error) = worker.interrupt() {
                emit_error(events, "http", error);
            }
            false
        }
        CommandMessage::Restart => {
            session.reset();
            if let Err(error) = worker.restart() {
                emit_error(events, "http", error);
            } else if let Err(error) = worker.send_kernel_info(&session.session_id) {
                emit_error(events, "websocket", error);
            }
            false
        }
        CommandMessage::Shutdown => true,
    }
}

fn handle_external_message(
    text: &str,
    worker: &mut ExternalWorker,
    session: &mut SessionState,
    events: &Sender<Value>,
) -> bool {
    let mut message: Value = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(error) => {
            emit_error(events, "websocket", error);
            return true;
        }
    };
    if message.get("channel").is_none() {
        message["channel"] = Value::String("iopub".to_string());
    }
    if message.get("msg_type").is_none() {
        if let Some(msg_type) = message
            .get("header")
            .and_then(|header| header.get("msg_type"))
            .cloned()
        {
            message["msg_type"] = msg_type;
        }
    }
    let became_ready = session.observe(&message);
    if events.send(message).is_err() {
        return false;
    }
    if became_ready {
        if events.send(ready_message()).is_err() {
            return false;
        }
        while let Some((msg_id, code)) = session.pending.pop_front() {
            if let Err(error) = worker.execute(&session.session_id, &msg_id, &code) {
                emit_error(events, "websocket", error);
                break;
            }
        }
    }
    true
}

fn request_value(
    session: &str,
    msg_id: &str,
    msg_type: &str,
    content: Value,
    parent_header: &Value,
) -> Value {
    json!({
        "header": request_header(session, msg_id, msg_type),
        "parent_header": parent_header,
        "metadata": {},
        "content": content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::Instant;
    use tempfile::tempdir;

    #[test]
    fn hmac_signature_is_lowercase_sha256_over_four_json_parts() {
        let parts = [
            b"one".to_vec(),
            b"two".to_vec(),
            b"three".to_vec(),
            b"four".to_vec(),
        ];
        let actual = sign_parts(b"secret", &parts);
        assert_eq!(
            actual,
            "4eb80409f5dc6fbe64a10e9da2bbb19a25d5b4206986f2f2a1bb76e12d182180"
        );
        assert_eq!(actual.len(), 64);
        assert!(
            actual
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
    }

    #[test]
    fn received_multipart_message_verifies_and_keeps_jupyter_fields() {
        let connection = ConnectionInfo {
            shell_port: 1,
            iopub_port: 2,
            stdin_port: 3,
            control_port: 4,
            ip: "127.0.0.1".to_string(),
            key: b"secret".to_vec(),
            transport: "tcp".to_string(),
            signature_scheme: "hmac-sha256".to_string(),
        };
        let header = json!({"msg_type":"execute_result","msg_id":"abc"});
        let parent = json!({"msg_id":"parent"});
        let metadata = json!({});
        let content = json!({"execution_count":1,"data":{"text/plain":"1"}});
        let parts = [
            serde_json::to_vec(&header).unwrap(),
            serde_json::to_vec(&parent).unwrap(),
            serde_json::to_vec(&metadata).unwrap(),
            serde_json::to_vec(&content).unwrap(),
        ];
        let mut frames = vec![
            MESSAGE_DELIMITER.to_vec(),
            sign_parts(&connection.key, &parts).into_bytes(),
        ];
        frames.extend(parts);
        frames.push(vec![0, 255]);
        let value = receive_message(&frames, &connection, "iopub").unwrap();
        assert_eq!(value["channel"], "iopub");
        assert_eq!(value["msg_type"], "execute_result");
        assert_eq!(value["content"]["data"]["text/plain"], "1");
        assert_eq!(value["buffers"][0], "AP8=");

        frames[1][0] = if frames[1][0] == b'0' { b'1' } else { b'0' };
        assert!(receive_message(&frames, &connection, "iopub").is_err());
    }

    #[test]
    fn connection_file_roundtrip_preserves_ports_and_key() {
        let path = std::env::temp_dir().join(format!("ipynb-kernel-test-{}.json", Uuid::new_v4()));
        let connection = ConnectionInfo {
            shell_port: 1001,
            iopub_port: 1002,
            stdin_port: 1003,
            control_port: 1004,
            ip: "127.0.0.1".to_string(),
            key: b"key".to_vec(),
            transport: "tcp".to_string(),
            signature_scheme: "hmac-sha256".to_string(),
        };
        write_connection_file(&path, &connection, "python3").unwrap();
        let loaded = read_connection_file(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(loaded.shell_port, 1001);
        assert_eq!(loaded.iopub_port, 1002);
        assert_eq!(loaded.stdin_port, 1003);
        assert_eq!(loaded.control_port, 1004);
        assert_eq!(loaded.key, b"key");
    }

    #[test]
    fn local_launch_preserves_kernelspec_argv_and_environment() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("record.sh");
        fs::write(
            &script,
            "#!/bin/sh\noutput=\"$3\"\n{ printf '%s\\n' \"$#\" \"$1\" \"$2\" \"$IPYNB_TEST_ENV\"; } > \"$output\"\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let spec_path = directory.path().join("kernel.json");
        fs::write(
            &spec_path,
            serde_json::to_vec(&json!({
                "argv": [
                    "/bin/sh",
                    script,
                    "{connection_file}",
                    "--connection={connection_file}",
                    "{resource_dir}/observed"
                ],
                "env": {"IPYNB_TEST_ENV": "${HOME}"},
                "language": "python"
            }))
            .unwrap(),
        )
        .unwrap();

        let config = LocalConfig {
            spec: read_kernel_spec(&spec_path, "test").unwrap(),
            // An absolute interpreter path belongs to the kernelspec and must
            // not be replaced by the selected Python executable.
            python: Some("/bin/false".to_string()),
            numpy_legacy_repr: false,
            connection_file: directory.path().join("connection.json"),
        };
        let (_connection, mut child) = launch_local(&config).unwrap();
        assert!(child.wait().unwrap().success());

        let observed = fs::read_to_string(directory.path().join("observed")).unwrap();
        let mut lines = observed.lines();
        assert_eq!(lines.next(), Some("3"));
        assert_eq!(lines.next(), Some(config.connection_file.to_str().unwrap()));
        let connection_argument = format!("--connection={}", config.connection_file.display());
        assert_eq!(lines.next(), Some(connection_argument.as_str()));
        let expected_env = std::env::var("HOME").ok();
        assert_eq!(lines.next(), expected_env.as_deref());
        assert_eq!(lines.next(), None);

        let _ = fs::remove_file(&config.connection_file);
    }

    #[test]
    fn pending_execution_is_fifo_and_only_flushes_after_ready() {
        let mut session = SessionState::new();
        session
            .pending
            .push_back(("one".to_string(), "1".to_string()));
        session
            .pending
            .push_back(("two".to_string(), "2".to_string()));
        let info = message_value(
            "shell",
            json!({"msg_type":"kernel_info_reply"}),
            json!({}),
            json!({}),
            json!({}),
        );
        let idle = message_value(
            "iopub",
            json!({"msg_type":"status"}),
            json!({}),
            json!({}),
            json!({"execution_state":"idle"}),
        );
        assert!(!session.observe(&info));
        assert!(session.observe(&idle));
        assert!(session.ready);
        assert_eq!(session.pending.pop_front().unwrap().0, "one");
        assert_eq!(session.pending.pop_front().unwrap().0, "two");
    }

    #[test]
    fn attached_kernel_executes_queued_code_over_signed_zmq() {
        let connection = ConnectionInfo {
            shell_port: free_port().unwrap(),
            iopub_port: free_port().unwrap(),
            stdin_port: free_port().unwrap(),
            control_port: free_port().unwrap(),
            ip: "127.0.0.1".to_string(),
            key: b"test-key".to_vec(),
            transport: "tcp".to_string(),
            signature_scheme: "hmac-sha256".to_string(),
        };
        let path = std::env::temp_dir().join(format!("ipynb-kernel-test-{}.json", Uuid::new_v4()));
        write_connection_file(&path, &connection, "python3").unwrap();

        let context = zmq::Context::new();
        let shell = context.socket(zmq::ROUTER).unwrap();
        let iopub = context.socket(zmq::PUB).unwrap();
        let control = context.socket(zmq::ROUTER).unwrap();
        shell
            .bind(&endpoint(&connection, connection.shell_port))
            .unwrap();
        iopub
            .bind(&endpoint(&connection, connection.iopub_port))
            .unwrap();
        control
            .bind(&endpoint(&connection, connection.control_port))
            .unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let key = connection.key.clone();
        let server = thread::spawn(move || {
            let mut poll_items = [
                shell.as_poll_item(zmq::POLLIN),
                control.as_poll_item(zmq::POLLIN),
            ];
            while !server_stop.load(Ordering::SeqCst) {
                if zmq::poll(&mut poll_items, 20).is_err() {
                    break;
                }
                if poll_items[0].is_readable() {
                    let frames = shell.recv_multipart(0).unwrap();
                    let delimiter = frames
                        .iter()
                        .position(|frame| frame.as_slice() == MESSAGE_DELIMITER)
                        .unwrap();
                    let identity = frames[delimiter - 1].clone();
                    let header: Value = serde_json::from_slice(&frames[delimiter + 2]).unwrap();
                    let content: Value = serde_json::from_slice(&frames[delimiter + 5]).unwrap();
                    let msg_type = header["msg_type"].as_str().unwrap();
                    if msg_type == "kernel_info_request" {
                        let reply = wire_frames(
                            &key,
                            json!({
                                "msg_id": Uuid::new_v4().to_string(),
                                "session": header["session"],
                                "msg_type": "kernel_info_reply",
                            }),
                            header.clone(),
                            json!({}),
                            json!({}),
                        );
                        let mut routed = vec![identity.clone()];
                        routed.extend(reply);
                        shell.send_multipart(routed, 0).unwrap();
                        thread::sleep(Duration::from_millis(60));
                        let status = wire_frames(
                            &key,
                            json!({
                                "msg_id": Uuid::new_v4().to_string(),
                                "session": header["session"],
                                "msg_type": "status",
                            }),
                            header,
                            json!({}),
                            json!({"execution_state":"idle"}),
                        );
                        iopub.send_multipart(status, 0).unwrap();
                    } else if msg_type == "execute_request" {
                        let busy = wire_frames(
                            &key,
                            json!({
                                "msg_id": Uuid::new_v4().to_string(),
                                "session": header["session"],
                                "msg_type": "status",
                            }),
                            header.clone(),
                            json!({}),
                            json!({"execution_state":"busy"}),
                        );
                        iopub.send_multipart(busy, 0).unwrap();
                        let result = wire_frames(
                            &key,
                            json!({
                                "msg_id": Uuid::new_v4().to_string(),
                                "session": header["session"],
                                "msg_type": "execute_result",
                            }),
                            header.clone(),
                            json!({}),
                            json!({
                                "execution_count": 1,
                                "data": {"text/plain": content["code"]},
                                "metadata": {},
                            }),
                        );
                        iopub.send_multipart(result, 0).unwrap();
                        let reply = wire_frames(
                            &key,
                            json!({
                                "msg_id": Uuid::new_v4().to_string(),
                                "session": header["session"],
                                "msg_type": "execute_reply",
                            }),
                            header.clone(),
                            json!({}),
                            json!({"status":"ok","execution_count":1}),
                        );
                        let mut routed = vec![identity];
                        routed.extend(reply);
                        shell.send_multipart(routed, 0).unwrap();
                        let idle = wire_frames(
                            &key,
                            json!({
                                "msg_id": Uuid::new_v4().to_string(),
                                "session": header["session"],
                                "msg_type": "status",
                            }),
                            header,
                            json!({}),
                            json!({"execution_state":"idle"}),
                        );
                        iopub.send_multipart(idle, 0).unwrap();
                    }
                }
            }
        });

        let kernel = Kernel::start(path.to_str().unwrap(), None, false).unwrap();
        let request = kernel.execute("2 + 2").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut messages = Vec::new();
        while Instant::now() < deadline {
            messages.extend(kernel.drain());
            if messages.iter().any(|message| {
                message["header"]["msg_type"] == "execute_result"
                    && message["parent_header"]["msg_id"] == request
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(messages.iter().any(|message| {
            message["header"]["msg_type"] == "execute_result"
                && message["parent_header"]["msg_id"] == request
                && message["content"]["data"]["text/plain"] == "2 + 2"
        }));
        kernel.shutdown().unwrap();
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        fs::remove_file(path).unwrap();
    }

    fn wire_frames(
        key: &[u8],
        header: Value,
        parent_header: Value,
        metadata: Value,
        content: Value,
    ) -> Vec<Vec<u8>> {
        let parts = [
            serde_json::to_vec(&header).unwrap(),
            serde_json::to_vec(&parent_header).unwrap(),
            serde_json::to_vec(&metadata).unwrap(),
            serde_json::to_vec(&content).unwrap(),
        ];
        let mut frames = vec![
            MESSAGE_DELIMITER.to_vec(),
            sign_parts(key, &parts).into_bytes(),
        ];
        frames.extend(parts);
        frames
    }
}
