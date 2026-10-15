//! Jupyter Server HTTP lifecycle and websocket transport.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use reqwest::Url;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket, connect};
use uuid::Uuid;

use super::session::SessionState;
use super::wire::request_header;
use super::worker::{CommandMessage, PreparedKernel, emit_error, emit_stopped, ready_message};

type ExternalSocket = WebSocket<MaybeTlsStream<TcpStream>>;

struct ExternalConfig {
    client: Client,
    base_url: Url,
    token: Option<String>,
}

pub(super) struct ExternalPrepared {
    config: ExternalConfig,
    kernel_id: String,
    socket: ExternalSocket,
}

struct ExternalWorker {
    config: ExternalConfig,
    kernel_id: String,
    socket: ExternalSocket,
}

pub(super) fn prepare_external(name: &str) -> Result<PreparedKernel> {
    let mut base_url = Url::parse(name).context("parse Jupyter Server URL")?;
    let token = base_url
        .query_pairs()
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned());
    base_url.set_query(None);
    base_url.set_fragment(None);

    let client = Client::builder()
        .timeout(Duration::from_secs(5))
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
    let mut websocket_request = websocket_url.as_str().into_client_request()?;
    if let Some(token) = &token {
        websocket_request
            .headers_mut()
            .insert("Authorization", format!("token {token}").parse()?);
    }
    let socket = match connect(websocket_request) {
        Ok((socket, _)) => socket,
        Err(error) => {
            let mut cleanup =
                client.delete(join_url(&base_url, &format!("api/kernels/{kernel_id}")));
            if let Some(token) = &token {
                cleanup = cleanup.header("Authorization", format!("token {token}"));
            }
            let _ = cleanup.send();
            return Err(error).context("connect Jupyter Server websocket");
        }
    };
    let socket = set_websocket_timeout(socket);
    Ok(PreparedKernel::External(Box::new(ExternalPrepared {
        config: ExternalConfig {
            client,
            base_url,
            token,
        },
        kernel_id,
        socket,
    })))
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
        self.socket.send(Message::Text(message.to_string()))?;
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
        self.socket.send(Message::Text(message.to_string()))?;
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

    fn send_kernel_info(&mut self, session: &str) -> Result<String> {
        let msg_id = Uuid::new_v4().to_string();
        let message = request_value(
            session,
            &msg_id,
            "kernel_info_request",
            json!({}),
            &json!({}),
        );
        self.socket.send(Message::Text(message.to_string()))?;
        Ok(msg_id)
    }
}

pub(super) fn run_external_worker(
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
    match worker.send_kernel_info(&session.session_id) {
        Ok(_) => session.probe_sent(Instant::now()),
        Err(error) => {
            emit_stopped(&events, "websocket", error);
            let _ = worker.shutdown();
            return;
        }
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

        if session.probe_due(Instant::now()) {
            match worker.send_kernel_info(&session.session_id) {
                Ok(_) => session.probe_sent(Instant::now()),
                Err(error) => {
                    emit_stopped(&events, "websocket", error);
                    break 'worker;
                }
            }
        }

        match worker.socket.read() {
            Ok(Message::Text(text)) => {
                if !handle_external_message(text.as_ref(), &mut session, &events, Instant::now()) {
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
                if !handle_external_message(text, &mut session, &events, Instant::now()) {
                    break 'worker;
                }
            }
            Ok(Message::Ping(payload)) => {
                if let Err(error) = worker.socket.send(Message::Pong(payload)) {
                    emit_stopped(&events, "websocket", error);
                    break 'worker;
                }
            }
            Ok(Message::Close(_)) => {
                emit_stopped(&events, "websocket", "Kernel websocket closed");
                break 'worker;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => {
                emit_stopped(&events, "websocket", error);
                break 'worker;
            }
        }
        if !finish_external_readiness(&mut worker, &mut session, &events) {
            break 'worker;
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
                    emit_stopped(events, "websocket", error);
                    return true;
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
                emit_stopped(events, "http", error);
                return true;
            } else {
                match worker.send_kernel_info(&session.session_id) {
                    Ok(_) => session.probe_sent(Instant::now()),
                    Err(error) => {
                        emit_stopped(events, "websocket", error);
                        return true;
                    }
                }
            }
            false
        }
        CommandMessage::Shutdown => true,
    }
}

fn handle_external_message(
    text: &str,
    session: &mut SessionState,
    events: &Sender<Value>,
    now: Instant,
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
    if message.get("msg_type").is_none()
        && let Some(msg_type) = message
            .get("header")
            .and_then(|header| header.get("msg_type"))
            .cloned()
    {
        message["msg_type"] = msg_type;
    }
    let channel = message
        .get("channel")
        .and_then(Value::as_str)
        .unwrap_or("iopub");
    session.observe(&message, channel, now);
    if events.send(message).is_err() {
        return false;
    }
    true
}

fn finish_external_readiness(
    worker: &mut ExternalWorker,
    session: &mut SessionState,
    events: &Sender<Value>,
) -> bool {
    if !session.maybe_ready(Instant::now()) {
        return true;
    }
    if events.send(ready_message()).is_err() {
        return false;
    }
    while let Some((msg_id, code)) = session.pending.pop_front() {
        if let Err(error) = worker.execute(&session.session_id, &msg_id, &code) {
            emit_stopped(events, "websocket", error);
            return false;
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
        "channel": if msg_type == "input_reply" { "stdin" } else { "shell" },
        "header": request_header(session, msg_id, msg_type),
        "parent_header": parent_header,
        "metadata": {},
        "content": content,
    })
}
