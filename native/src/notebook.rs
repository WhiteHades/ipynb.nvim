use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use tempfile::NamedTempFile;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

const NANOS_PER_SECOND: i128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Mtime {
    pub sec: i64,
    pub nsec: i64,
}

#[cfg(unix)]
fn metadata_mtime(metadata: &fs::Metadata) -> Mtime {
    Mtime {
        sec: metadata.mtime(),
        nsec: metadata.mtime_nsec(),
    }
}

#[cfg(not(unix))]
fn metadata_mtime(metadata: &fs::Metadata) -> Result<Mtime> {
    let modified = metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?;
    Ok(Mtime {
        sec: i64::try_from(modified.as_secs()).context("file mtime is out of range")?,
        nsec: i64::from(modified.subsec_nanos()),
    })
}

fn file_mtime(path: &Path) -> Result<Option<Mtime>> {
    match fs::metadata(path) {
        Ok(metadata) => {
            #[cfg(unix)]
            let value = metadata_mtime(&metadata);
            #[cfg(not(unix))]
            let value = metadata_mtime(&metadata)?;
            Ok(Some(value))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not stat {}", path.display())),
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn write_target(path: &Path) -> Result<PathBuf> {
    let absolute = absolute_path(path)?;
    match fs::symlink_metadata(&absolute) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::canonicalize(&absolute)
            .with_context(|| format!("could not resolve notebook symlink {}", path.display())),
        Ok(_) => Ok(absolute),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(absolute),
        Err(error) => Err(error).with_context(|| format!("could not inspect {}", path.display())),
    }
}

fn number(value: &Value, name: &str) -> Result<i64> {
    if let Some(value) = value.as_i64() {
        return Ok(value);
    }
    if let Some(value) = value.as_u64() {
        return i64::try_from(value).with_context(|| format!("{name} is out of range"));
    }
    if let Some(value) = value.as_str() {
        return value
            .parse()
            .with_context(|| format!("{name} is not an integer"));
    }
    bail!("{name} must be an integer")
}

fn nanos_mtime(value: i128) -> Result<Mtime> {
    let sec = value.div_euclid(NANOS_PER_SECOND);
    let nsec = value.rem_euclid(NANOS_PER_SECOND);
    Ok(Mtime {
        sec: i64::try_from(sec).context("mtime seconds are out of range")?,
        nsec: i64::try_from(nsec).context("mtime nanoseconds are out of range")?,
    })
}

fn expected_mtime(value: Option<&Value>) -> Result<Option<Mtime>> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let value = value.get("mtime").unwrap_or(value);
    if let Some(object) = value.as_object() {
        let sec = number(
            object.get("sec").context("mtime.sec is missing")?,
            "mtime.sec",
        )?;
        let nsec = number(
            object.get("nsec").context("mtime.nsec is missing")?,
            "mtime.nsec",
        )?;
        if !(0..1_000_000_000).contains(&nsec) {
            bail!("mtime.nsec is out of range");
        }
        return Ok(Some(Mtime { sec, nsec }));
    }
    if let Some(value) = value.as_i64() {
        return nanos_mtime(i128::from(value)).map(Some);
    }
    if let Some(value) = value.as_u64() {
        return nanos_mtime(i128::from(value)).map(Some);
    }
    if let Some(value) = value.as_str() {
        return value
            .parse::<i128>()
            .with_context(|| "mtime is not an integer")
            .and_then(nanos_mtime)
            .map(Some);
    }
    bail!("mtime must be {{\"sec\",\"nsec\"}} or integer nanoseconds")
}

fn check_mtime(path: &Path, expected: Option<Mtime>) -> Result<()> {
    if let Some(expected) = expected {
        let actual = file_mtime(path)?;
        if actual != Some(expected) {
            bail!("notebook changed on disk since it was opened");
        }
    }
    Ok(())
}

pub fn read_json(path: &Path) -> Result<Value> {
    let bytes =
        fs::read(path).with_context(|| format!("could not read notebook {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("could not parse notebook {}", path.display()))
}

fn split_text(value: &mut Value) {
    let Value::String(source) = value else {
        return;
    };
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = source.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if matches!(
            ch,
            '\n' | '\r' | '\x0b' | '\x0c' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}'
        ) {
            let mut end = index + ch.len_utf8();
            if ch == '\r' && chars.peek().is_some_and(|(_, ch)| *ch == '\n') {
                end = chars.next().unwrap().0 + 1;
            }
            lines.push(json!(&source[start..end]));
            start = end;
        }
    }
    if start < source.len() {
        lines.push(json!(&source[start..]));
    }
    *value = Value::Array(lines);
}
fn split_mime(data: &mut Value) {
    if let Some(data) = data.as_object_mut() {
        for (kind, value) in data {
            if kind.starts_with("text/")
                || matches!(kind.as_str(), "image/svg+xml" | "application/javascript")
            {
                split_text(value);
            }
        }
    }
}
pub fn split_output(output: &mut Value) {
    match output["output_type"].as_str() {
        Some("stream") => split_text(&mut output["text"]),
        Some("execute_result" | "display_data") => split_mime(&mut output["data"]),
        _ => {}
    }
}

// Match nbformat's line-oriented disk representation without a Python round trip.
pub fn split_lines(notebook: &mut Value) {
    if let Some(cells) = notebook["cells"].as_array_mut() {
        for cell in cells {
            split_text(&mut cell["source"]);
            if let Some(attachments) = cell.get_mut("attachments").and_then(Value::as_object_mut) {
                for data in attachments.values_mut() {
                    split_mime(data);
                }
            }
            if let Some(outputs) = cell.get_mut("outputs").and_then(Value::as_array_mut) {
                for output in outputs {
                    split_output(output);
                }
            }
        }
    }
}

pub fn write_atomic(path: &Path, notebook: &Value, expected: Option<&Value>) -> Result<Value> {
    let target = write_target(path)?;
    let expected = expected_mtime(expected)?;
    check_mtime(&target, expected)?;

    let parent = target
        .parent()
        .context("notebook path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let mode = match fs::metadata(&target) {
        Ok(metadata) => {
            #[cfg(unix)]
            {
                Some(metadata.permissions().mode() & 0o7777)
            }
            #[cfg(not(unix))]
            {
                None
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("could not inspect {}", target.display()));
        }
    };

    let mut temporary = NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "could not create temporary notebook in {}",
            parent.display()
        )
    })?;
    {
        let mut output = BufWriter::new(temporary.as_file_mut());
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
        notebook
            .serialize(&mut serde_json::Serializer::with_formatter(
                &mut output,
                formatter,
            ))
            .context("could not serialize notebook")?;
        output
            .write_all(b"\n")
            .context("could not terminate notebook")?;
        output
            .flush()
            .context("could not write temporary notebook")?;
    }
    temporary
        .as_file()
        .sync_all()
        .context("could not fsync temporary notebook")?;
    if let Some(mode) = mode {
        #[cfg(unix)]
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(mode))
            .context("could not preserve notebook permissions")?;
    }

    // The conversion can take long enough for another process to modify the
    // file. Check once more immediately before the replacement.
    check_mtime(&target, expected)?;
    temporary
        .persist(&target)
        .map_err(|error| error.error)
        .with_context(|| format!("could not atomically replace {}", target.display()))?;
    File::open(parent)
        .with_context(|| format!("could not open {} for fsync", parent.display()))?
        .sync_all()
        .with_context(|| format!("could not fsync {}", parent.display()))?;

    let mtime = file_mtime(&target)?.context("notebook disappeared after atomic write")?;
    Ok(json!({"mtime": mtime}))
}

pub struct Converter {
    child: Child,
    input: Option<ChildStdin>,
    output: Option<BufReader<ChildStdout>>,
}

impl Converter {
    pub fn new(python: &str, script: &Path) -> Result<Self> {
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
            input: Some(input),
            output: Some(BufReader::new(output)),
        })
    }

    pub fn call(&mut self, request: &Value) -> Result<Value> {
        let payload =
            serde_json::to_string(request).context("could not encode converter request")?;
        let input = self.input.as_mut().context("converter stdin is closed")?;
        input
            .write_all(payload.as_bytes())
            .context("could not write converter request")?;
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
        let response: Value = serde_json::from_str(&line).context("invalid converter response")?;
        if response["ok"] != true {
            bail!(
                "notebook converter failed: {}",
                response["error"]
                    .as_str()
                    .unwrap_or("unknown converter error")
            );
        }
        Ok(response["result"].clone())
    }

    pub fn read(&mut self, path: &Path, template: &Path) -> Result<Value> {
        self.call(&json!({
            "op": "read",
            "path": path.to_string_lossy(),
            "template": template.to_string_lossy(),
        }))
    }

    pub fn convert(&mut self, text: &str, existing: Option<Value>) -> Result<Value> {
        self.call(&json!({"op": "convert", "text": text, "existing": existing}))
    }
}

impl Drop for Converter {
    fn drop(&mut self) {
        if let Some(mut input) = self.input.take() {
            let _ = input.write_all(
                br#"{"op":"shutdown"}
"#,
            );
            let _ = input.flush();
        }
        self.output.take();
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_roundtrip_conflict_and_failure_preserve_files() -> Result<()> {
        let directory = tempdir()?;
        let path = directory.path().join("notebook.ipynb");
        let mut original = json!({"cells": [{"source":"α\r\nβ\u{2028}last", "cell_type":"code",
            "outputs":[{"output_type":"stream","name":"stdout","text":"1\n2\n"}]}],
            "metadata": {}, "nbformat": 4, "nbformat_minor": 5});
        split_lines(&mut original);
        assert_eq!(
            original["cells"][0]["source"],
            json!(["α\r\n", "β\u{2028}", "last"])
        );
        assert_eq!(
            original["cells"][0]["outputs"][0]["text"],
            json!(["1\n", "2\n"])
        );
        let once = original.clone();
        split_lines(&mut original);
        assert_eq!(original, once);
        let result = write_atomic(&path, &original, None)?;
        assert_eq!(read_json(&path)?, original);

        let mtime = result
            .get("mtime")
            .context("missing returned mtime")?
            .clone();
        fs::write(&path, b"{\"changed\":true}\n")?;
        let error =
            write_atomic(&path, &original, Some(&mtime)).expect_err("stale mtime must conflict");
        assert!(error.to_string().contains("changed on disk"));
        assert_eq!(fs::read_to_string(&path)?, "{\"changed\":true}\n");

        let blocked = directory.path().join("directory-target");
        fs::create_dir(&blocked)?;
        assert!(write_atomic(&blocked, &original, None).is_err());
        assert!(blocked.is_dir());
        Ok(())
    }
}
