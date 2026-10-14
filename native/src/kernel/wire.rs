//! ZMQ sockets and signed Jupyter multipart messages.

use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use chrono::{SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

use super::local::ConnectionInfo;

type HmacSha256 = Hmac<Sha256>;

pub(super) const MESSAGE_DELIMITER: &[u8] = b"<IDS|MSG>";
const PROTOCOL_VERSION: &str = "5.3";

pub(super) fn command_wakeup(context: &zmq::Context) -> Result<(zmq::Socket, zmq::Socket)> {
    let endpoint = format!("inproc://ipynb-command-{}", Uuid::new_v4());
    let receiver = context.socket(zmq::PAIR)?;
    receiver.set_linger(0)?;
    receiver.set_rcvhwm(1)?;
    receiver.bind(&endpoint)?;
    let sender = context.socket(zmq::PAIR)?;
    sender.set_linger(0)?;
    sender.set_sndhwm(1)?;
    sender.connect(&endpoint)?;
    Ok((sender, receiver))
}

pub(super) struct ZmqTransport {
    _context: zmq::Context,
    shell: zmq::Socket,
    iopub: zmq::Socket,
    stdin: zmq::Socket,
    control: zmq::Socket,
    connection: ConnectionInfo,
}

impl ZmqTransport {
    pub(super) fn connect(context: &zmq::Context, connection: ConnectionInfo) -> Result<Self> {
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
        let identity = Uuid::new_v4().to_string();
        let shell = connect_socket(
            context,
            zmq::DEALER,
            &connection,
            connection.shell_port,
            identity.as_bytes(),
        )?;
        let iopub = connect_iopub(context, &connection, connection.iopub_port)?;
        let stdin = connect_socket(
            context,
            zmq::DEALER,
            &connection,
            connection.stdin_port,
            identity.as_bytes(),
        )?;
        let control = connect_socket(
            context,
            zmq::DEALER,
            &connection,
            connection.control_port,
            identity.as_bytes(),
        )?;
        Ok(Self {
            _context: context.clone(),
            shell,
            iopub,
            stdin,
            control,
            connection,
        })
    }

    pub(super) fn sockets(&self) -> [(&zmq::Socket, &'static str); 4] {
        [
            (&self.iopub, "iopub"),
            (&self.shell, "shell"),
            (&self.stdin, "stdin"),
            (&self.control, "control"),
        ]
    }

    pub(super) fn receive(&self, frames: &[Vec<u8>], channel: &str) -> Result<Value> {
        receive_message(frames, &self.connection, channel)
    }

    pub(super) fn send_kernel_info(&self, session: &str) -> Result<String> {
        send_request(
            &self.shell,
            &self.connection,
            session,
            "kernel_info_request",
            json!({}),
            &json!({}),
        )
    }

    pub(super) fn execute(&self, session: &str, msg_id: &str, code: &str) -> Result<()> {
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

    pub(super) fn input(&self, session: &str, parent: &Value, value: &str) -> Result<()> {
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

    pub(super) fn interrupt(&self, session: &str) -> Result<()> {
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

    pub(super) fn shutdown(&self, session: &str, restart: bool) -> Result<()> {
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

pub(super) fn endpoint(connection: &ConnectionInfo, port: u16) -> String {
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

pub(super) fn request_header(session: &str, msg_id: &str, msg_type: &str) -> Value {
    json!({
        "msg_id": msg_id,
        "username": "ipynb.nvim",
        "session": session,
        "date": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "msg_type": msg_type,
        "version": PROTOCOL_VERSION,
    })
}

pub(super) fn sign_parts(key: &[u8], parts: &[Vec<u8>; 4]) -> String {
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

pub(super) fn receive_message(
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

pub(super) fn message_value(
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
