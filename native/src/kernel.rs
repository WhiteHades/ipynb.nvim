//! Synchronous Jupyter kernel transport.
//!
//! Every transport socket belongs to one worker thread. Callers submit
//! commands and drain ordinary JSON messages, leaving UI and Neovim concerns
//! to the adapter layer.

mod local;
mod remote;
mod session;
mod wire;
mod worker;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use uuid::Uuid;

use wire::command_wakeup;
use worker::{CommandMessage, prepare_kernel, runtime_error, worker_main};

/// A running Jupyter kernel.
pub struct Kernel {
    commands: Sender<CommandMessage>,
    wake: Option<zmq::Socket>,
    events: Receiver<Value>,
    worker: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
    stopped_reported: AtomicBool,
    restartable: bool,
    language: Option<String>,
}

impl Kernel {
    /// Start a local kernelspec, attach to a connection file, or create a
    /// kernel through a Jupyter Server URL.
    pub fn start(
        name: &str,
        python: Option<&str>,
        numpy_legacy_repr: bool,
        spec_path: Option<&Path>,
        cwd: Option<&Path>,
    ) -> Result<Self> {
        let (wake, context, wake_receiver) =
            if name.starts_with("http://") || name.starts_with("https://") {
                (None, None, None)
            } else {
                let context = zmq::Context::new();
                let (wake, receiver) = command_wakeup(&context)?;
                (Some(wake), Some(context), Some(receiver))
            };
        let prepared = prepare_kernel(name, python, numpy_legacy_repr, spec_path, cwd)?;
        let restartable = prepared.restartable();
        let language = prepared.language();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("ipynb-jupyter-kernel".to_string())
            .spawn(move || worker_main(prepared, command_rx, event_tx, context, wake_receiver))
            .context("spawn Jupyter kernel worker")?;

        Ok(Self {
            commands: command_tx,
            wake,
            events: event_rx,
            worker: Mutex::new(Some(worker)),
            closed: AtomicBool::new(false),
            stopped_reported: AtomicBool::new(false),
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
        self.notify_worker();
        Ok(msg_id)
    }

    /// Reply to the most recent input request.
    pub fn stdin(&self, value: &str) -> Result<()> {
        self.ensure_open()?;
        self.commands
            .send(CommandMessage::Input(value.to_string()))
            .context("send stdin reply to kernel worker")?;
        self.notify_worker();
        Ok(())
    }

    /// Interrupt the currently running execution.
    pub fn interrupt(&self) -> Result<()> {
        self.ensure_open()?;
        self.commands
            .send(CommandMessage::Interrupt)
            .context("send interrupt request to kernel worker")?;
        self.notify_worker();
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
        self.notify_worker();
        Ok(())
    }

    /// Stop the worker and release owned process/file resources.
    pub fn shutdown(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        // A stopped worker has already released its resources; still join it.
        let _ = self.commands.send(CommandMessage::Shutdown);
        self.notify_worker();
        self.join_worker()
    }

    /// Return every message currently available without waiting.
    pub fn drain(&self) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(message) => {
                    if message["msg_type"] == "kernel_stopped" {
                        self.stopped_reported.store(true, Ordering::SeqCst);
                    }
                    messages.push(message);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.stopped_reported.swap(true, Ordering::SeqCst)
                        && !self.closed.load(Ordering::SeqCst)
                    {
                        messages.push(runtime_error(
                            "kernel_stopped",
                            "runtime",
                            "Kernel worker stopped",
                        ));
                    }
                    break;
                }
            }
        }
        messages
    }

    pub fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst)
            || self.stopped_reported.load(Ordering::SeqCst)
            || self
                .worker
                .lock()
                .is_ok_and(|worker| worker.as_ref().is_some_and(JoinHandle::is_finished))
        {
            Err(anyhow!("Jupyter kernel is stopped; initialize it again"))
        } else {
            Ok(())
        }
    }

    fn notify_worker(&self) {
        if let Some(wake) = &self.wake {
            // The command queue remains the source of truth. A full socket is
            // a coalesced notification, so a nonblocking send cannot lose work.
            let _ = wake.send(&[0u8][..], zmq::DONTWAIT);
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
            self.notify_worker();
        }
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}
