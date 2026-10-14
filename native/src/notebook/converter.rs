//! Jupytext and optional renderers are started only when a caller needs them.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

pub struct Converter {
    python: String,
    script: PathBuf,
    process: Option<Process>,
}

struct Process {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
    responses: Option<Receiver<io::Result<String>>>,
    reader: Option<JoinHandle<()>>,
    _scratch: tempfile::TempDir,
}

impl Converter {
    pub fn new(python: &str, script: &Path) -> Self {
        Self {
            python: python.into(),
            script: script.into(),
            process: None,
        }
    }

    pub fn call(&mut self, request: &Value) -> Result<Value> {
        self.call_with_timeout(request, Duration::from_secs(20))
    }

    pub fn warmup(&mut self) -> Result<()> {
        if self.process.is_none() {
            self.process = Some(Process::start(&self.python, &self.script)?);
        }
        Ok(())
    }

    fn call_with_timeout(&mut self, request: &Value, timeout: Duration) -> Result<Value> {
        self.warmup()?;
        let mut response = match self.process.as_mut().unwrap().call(request, timeout) {
            Ok(response) => response,
            Err(error) => {
                self.process.take();
                return Err(error);
            }
        };
        if response["ok"] == false {
            bail!(
                "notebook converter failed: {}",
                response["error"]
                    .as_str()
                    .unwrap_or("unknown converter error")
            );
        }
        Ok(response["result"].take())
    }

    pub fn read(&mut self, path: &Path, template: &Path) -> Result<Value> {
        self.call(&json!({"op": "read", "path": path.to_string_lossy(), "template": template.to_string_lossy()}))
    }

    #[cfg(test)]
    pub fn convert(&mut self, text: &str, existing: Option<Value>) -> Result<Value> {
        self.convert_at(text, existing, None)
    }

    pub fn convert_at(
        &mut self,
        text: &str,
        existing: Option<Value>,
        path: Option<&Path>,
    ) -> Result<Value> {
        self.call(&json!({"op": "convert", "text": text, "existing": existing, "path": path}))
    }
}

impl Process {
    fn start(python: &str, script: &Path) -> Result<Self> {
        let scratch =
            tempfile::tempdir().context("could not create converter scratch directory")?;
        let mut command = Command::new(python);
        command
            .arg("-u")
            .arg(script)
            .env("IPYNB_CONVERTER_TMPDIR", scratch.path())
            .env("TMPDIR", scratch.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        command.process_group(0);
        let child = command
            .spawn()
            .with_context(|| format!("could not start notebook converter {python}"))?;
        let mut process = Self {
            child,
            input: None,
            responses: None,
            reader: None,
            _scratch: scratch,
        };
        let input = process
            .child
            .stdin
            .take()
            .context("converter stdin was not piped")?;
        process.input = Some(BufWriter::new(input));
        let output = process
            .child
            .stdout
            .take()
            .context("converter stdout was not piped")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        process.responses = Some(receiver);
        process.reader = Some(
            thread::Builder::new()
                .spawn(move || {
                    let mut output = BufReader::new(output);
                    loop {
                        let mut line = String::new();
                        let response = match output.read_line(&mut line) {
                            Ok(0) => Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "notebook converter exited",
                            )),
                            Ok(_) => Ok(line),
                            Err(error) => Err(error),
                        };
                        let failed = response.is_err();
                        if sender.send(response).is_err() || failed {
                            break;
                        }
                    }
                })
                .context("could not start converter response reader")?,
        );
        Ok(process)
    }

    fn call(&mut self, request: &Value, timeout: Duration) -> Result<Value> {
        let input = self.input.as_mut().context("converter stdin is closed")?;
        serde_json::to_writer(&mut *input, request).context("could not write converter request")?;
        input
            .write_all(b"\n")
            .context("could not terminate converter request")?;
        input.flush().context("could not flush converter request")?;
        let responses = self
            .responses
            .as_ref()
            .context("converter stdout is closed")?;
        let line = match responses.recv_timeout(timeout) {
            Ok(response) => response.context("could not read converter response")?,
            Err(mpsc::RecvTimeoutError::Timeout) => bail!("notebook converter response timed out"),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("notebook converter response reader stopped")
            }
        };
        let response: Value = serde_json::from_str(&line).context("invalid converter response")?;
        if !response["ok"].is_boolean() {
            bail!("invalid converter response: missing boolean ok");
        }
        Ok(response)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: process_group(0) makes the child's PID its owned process group ID.
            unsafe { kill(-(self.child.id() as i32), 9) };
        }
        #[cfg(not(unix))]
        let _ = self.child.kill();
        self.input.take();
        self.responses.take();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_starts_on_demand_and_reports_startup_failure() {
        let mut converter = Converter::new("/missing/ipynb/python", Path::new("converter.py"));
        assert!(converter.process.is_none());
        assert!(
            converter
                .convert("", None)
                .unwrap_err()
                .to_string()
                .contains("could not start notebook converter")
        );
        assert!(converter.process.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_group_cleans_scratch_and_restarts() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;

        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("fake-converter");
        fs::write(
            &script,
            "#!/bin/sh\n\
             IFS= read -r request || exit\n\
             sleep 60 &\n\
             echo $! > \"$IPYNB_CONVERTER_TMPDIR/child.pid\"\n\
             printf '%s\\n' '{\"ok\":true,\"result\":\"ready\"}'\n\
             IFS= read -r request || exit\n\
             wait\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let mut converter = Converter::new(script.to_str().unwrap(), Path::new("unused"));
        converter.warmup().unwrap();
        let warmed_pid = converter.process.as_ref().unwrap().child.id();
        converter.warmup().unwrap();
        assert_eq!(converter.call(&json!({})).unwrap(), "ready");
        let process = converter.process.as_ref().unwrap();
        assert_eq!(process.child.id(), warmed_pid);
        let group = process.child.id() as i32;
        let scratch = process._scratch.path().to_owned();
        let descendant: i32 = fs::read_to_string(scratch.join("child.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        let started = Instant::now();
        let error = converter
            .call_with_timeout(&json!({}), Duration::from_millis(100))
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(converter.process.is_none());
        assert!(!scratch.exists());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            // SAFETY: signal 0 only checks whether the recorded process/group exists.
            let gone = unsafe { kill(-group, 0) != 0 && kill(descendant, 0) != 0 };
            if gone {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "converter process group survived"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(converter.call(&json!({})).unwrap(), "ready");
        let restarted_scratch = converter
            .process
            .as_ref()
            .unwrap()
            ._scratch
            .path()
            .to_owned();
        assert_ne!(scratch, restarted_scratch);
        drop(converter);
        assert!(!restarted_scratch.exists());
    }
}
