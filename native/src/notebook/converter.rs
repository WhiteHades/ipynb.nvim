//! Jupytext and optional renderers are started only when a caller needs them.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct Converter {
    python: String,
    script: PathBuf,
    process: Option<Process>,
}

struct Process {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
    output: Option<BufReader<ChildStdout>>,
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
        if self.process.is_none() {
            self.process = Some(Process::start(&self.python, &self.script)?);
        }
        self.process.as_mut().unwrap().call(request)
    }

    pub fn read(&mut self, path: &Path, template: &Path) -> Result<Value> {
        self.call(&json!({"op": "read", "path": path.to_string_lossy(), "template": template.to_string_lossy()}))
    }

    pub fn convert(&mut self, text: &str, existing: Option<Value>) -> Result<Value> {
        self.call(&json!({"op": "convert", "text": text, "existing": existing}))
    }
}

impl Process {
    fn start(python: &str, script: &Path) -> Result<Self> {
        let mut child = Command::new(python)
            .arg("-u")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("could not start notebook converter {python}"))?;
        let input = child
            .stdin
            .take()
            .context("converter stdin was not piped")?;
        let output = child
            .stdout
            .take()
            .context("converter stdout was not piped")?;
        Ok(Self {
            child,
            input: Some(BufWriter::new(input)),
            output: Some(BufReader::new(output)),
        })
    }

    fn call(&mut self, request: &Value) -> Result<Value> {
        let input = self.input.as_mut().context("converter stdin is closed")?;
        serde_json::to_writer(&mut *input, request).context("could not write converter request")?;
        input
            .write_all(b"\n")
            .context("could not terminate converter request")?;
        input.flush().context("could not flush converter request")?;
        let output = self.output.as_mut().context("converter stdout is closed")?;
        let mut line = String::new();
        if output
            .read_line(&mut line)
            .context("could not read converter response")?
            == 0
        {
            let status = self.child.try_wait().ok().flatten();
            bail!(
                "notebook converter exited{}",
                status.map(|s| format!(" ({s})")).unwrap_or_default()
            );
        }
        let mut response: Value =
            serde_json::from_str(&line).context("invalid converter response")?;
        if response["ok"] != true {
            bail!(
                "notebook converter failed: {}",
                response["error"]
                    .as_str()
                    .unwrap_or("unknown converter error")
            );
        }
        Ok(response["result"].take())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut input) = self.input.take() {
            let _ = input.write_all(b"{\"op\":\"shutdown\"}\n");
            let _ = input.flush();
        }
        self.output.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
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
}
