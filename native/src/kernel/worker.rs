//! Worker dispatch and owned local-kernel lifecycle.

use std::fs;
use std::path::Path;
use std::process::Child;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use super::local::{
    ConnectionInfo, LocalConfig, LocalPrepared, launch_local, prepare_local, read_connection_file,
};
use super::remote::{ExternalPrepared, prepare_external, run_external_worker};
use super::session::SessionState;
use super::wire::ZmqTransport;

pub(super) enum CommandMessage {
    Execute { msg_id: String, code: String },
    Input(String),
    Interrupt,
    Restart,
    Shutdown,
}

pub(super) enum PreparedKernel {
    Local(Box<LocalPrepared>),
    Attached { connection: ConnectionInfo },
    External(Box<ExternalPrepared>),
}

impl PreparedKernel {
    pub(super) fn restartable(&self) -> bool {
        !matches!(self, Self::Attached { .. })
    }

    pub(super) fn language(&self) -> Option<String> {
        match self {
            Self::Local(local) => Some(local.config.language().to_string()),
            Self::Attached { .. } | Self::External(_) => None,
        }
    }
}

pub(super) fn prepare_kernel(
    name: &str,
    python: Option<&str>,
    numpy_legacy_repr: bool,
    spec_path: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<PreparedKernel> {
    if name.starts_with("http://") || name.starts_with("https://") {
        return prepare_external(name);
    }
    if let Some(spec_path) = spec_path {
        return prepare_local(name, python, numpy_legacy_repr, spec_path, cwd)
            .map(|local| PreparedKernel::Local(Box::new(local)));
    }

    let path = Path::new(name);
    if path.exists() || name.ends_with(".json") {
        return Ok(PreparedKernel::Attached {
            connection: read_connection_file(path)
                .with_context(|| format!("read connection file {}", path.display()))?,
        });
    }

    Err(anyhow!(
        "local kernel {name} requires an authoritative kernelspec path"
    ))
}

struct ZmqWorker {
    transport: ZmqTransport,
    context: zmq::Context,
    wake: zmq::Socket,
    child: Option<Child>,
    local_config: Option<LocalConfig>,
}

impl ZmqWorker {
    fn from_prepared(
        prepared: PreparedKernel,
        context: zmq::Context,
        wake: zmq::Socket,
    ) -> Result<Self> {
        match prepared {
            PreparedKernel::Local(local) => {
                let LocalPrepared {
                    config,
                    mut child,
                    connection,
                } = *local;
                let transport = match ZmqTransport::connect(&context, connection) {
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
                    context,
                    wake,
                    child: Some(child),
                    local_config: Some(config),
                })
            }
            PreparedKernel::Attached { connection } => Ok(Self {
                transport: ZmqTransport::connect(&context, connection)?,
                context,
                wake,
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

    fn restart(&mut self, session: &str) -> Result<String> {
        let config = self
            .local_config
            .as_ref()
            .ok_or_else(|| anyhow!("cannot restart an attached connection-file kernel"))?;
        self.transport.shutdown(session, false).ok();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let (connection, mut child) = launch_local(config)?;
        let transport = match ZmqTransport::connect(&self.context, connection) {
            Ok(transport) => transport,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&config.connection_file);
                return Err(error);
            }
        };
        self.transport = transport;
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

pub(super) fn worker_main(
    prepared: PreparedKernel,
    commands: Receiver<CommandMessage>,
    events: Sender<Value>,
    context: Option<zmq::Context>,
    wake: Option<zmq::Socket>,
) {
    match prepared {
        PreparedKernel::External(external) => run_external_worker(*external, commands, events),
        prepared => match (context, wake) {
            (Some(context), Some(wake)) => {
                match ZmqWorker::from_prepared(prepared, context, wake) {
                    Ok(worker) => run_zmq_worker(worker, commands, events),
                    Err(error) => emit_stopped(&events, "startup", error),
                }
            }
            _ => emit_stopped(
                &events,
                "startup",
                "local ZMQ kernel has no command wake socket",
            ),
        },
    }
}

fn run_zmq_worker(
    mut worker: ZmqWorker,
    commands: Receiver<CommandMessage>,
    events: Sender<Value>,
) {
    let mut session = SessionState::new();
    match worker.transport.send_kernel_info(&session.session_id) {
        Ok(_) => session.probe_sent(Instant::now()),
        Err(error) => {
            emit_stopped(&events, "shell", error);
            worker.shutdown(&session.session_id);
            return;
        }
    }

    let mut last_child_check = Instant::now();
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

        let now = Instant::now();
        if now.duration_since(last_child_check) >= Duration::from_millis(250) {
            last_child_check = now;
            if let Some(child) = &mut worker.child
                && let Ok(Some(status)) = child.try_wait()
            {
                emit_stopped(&events, "process", format!("Kernel exited: {status}"));
                break;
            }
        }
        if session.probe_due(now) {
            match worker.transport.send_kernel_info(&session.session_id) {
                Ok(_) => session.probe_sent(now),
                Err(error) => {
                    emit_stopped(&events, "shell", error);
                    break;
                }
            }
        }
        let poll_timeout = if session.ready { 250 } else { 20 };
        let sockets = worker.transport.sockets();
        let mut poll_items = [
            worker.wake.as_poll_item(zmq::POLLIN),
            sockets[0].0.as_poll_item(zmq::POLLIN),
            sockets[1].0.as_poll_item(zmq::POLLIN),
            sockets[2].0.as_poll_item(zmq::POLLIN),
            sockets[3].0.as_poll_item(zmq::POLLIN),
        ];
        if let Err(error) = zmq::poll(&mut poll_items, poll_timeout) {
            emit_stopped(&events, "transport", error);
            break;
        }
        if poll_items[0].get_revents().contains(zmq::POLLIN) {
            while worker.wake.recv_bytes(zmq::DONTWAIT).is_ok() {}
        }
        for (index, (socket, channel)) in sockets.iter().enumerate() {
            if !poll_items[index + 1].get_revents().contains(zmq::POLLIN) {
                continue;
            }
            let frames = match socket.recv_multipart(0) {
                Ok(frames) => frames,
                Err(error) => {
                    emit_error(&events, channel, error);
                    continue;
                }
            };
            match worker.transport.receive(&frames, channel) {
                Ok(message) => {
                    session.observe(&message, channel, Instant::now());
                    if events.send(message).is_err() {
                        break 'worker;
                    }
                }
                Err(error) => emit_error(&events, channel, error),
            }
        }
        if !finish_zmq_readiness(&mut worker, &mut session, &events) {
            break;
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
                    emit_stopped(events, "shell", error);
                    return true;
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
            match worker.restart(&session.session_id) {
                Ok(_) => session.probe_sent(Instant::now()),
                Err(error) => {
                    emit_stopped(events, "restart", error);
                    return true;
                }
            }
            false
        }
        CommandMessage::Shutdown => true,
    }
}

fn finish_zmq_readiness(
    worker: &mut ZmqWorker,
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
            emit_stopped(events, "shell", error);
            return false;
        }
    }
    true
}

pub(super) fn ready_message() -> Value {
    json!({
        "channel": "runtime",
        "msg_type": "ready",
        "header": { "msg_type": "ready" },
        "parent_header": {},
        "metadata": {},
        "content": {},
    })
}

pub(super) fn emit_error(events: &Sender<Value>, channel: &str, error: impl std::fmt::Display) {
    let _ = events.send(runtime_error("transport_error", channel, error));
}

pub(super) fn emit_stopped(events: &Sender<Value>, channel: &str, error: impl std::fmt::Display) {
    let _ = events.send(runtime_error("kernel_stopped", channel, error));
}

pub(super) fn runtime_error(kind: &str, channel: &str, error: impl std::fmt::Display) -> Value {
    json!({
        "channel": channel,
        "msg_type": kind,
        "header": { "msg_type": kind },
        "parent_header": {},
        "metadata": {},
        "content": { "error": error.to_string() },
    })
}
