use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

mod converter;
pub use converter::Converter;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

const NANOS_PER_SECOND: i128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

pub(crate) fn file_mtime(path: &Path) -> Result<Option<Mtime>> {
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

fn checked_digest(value: &str) -> Result<&str> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("expected digest must be a 64-character SHA-256 hex string");
    }
    Ok(value)
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
    write_file_atomic(path, expected, false, |output| {
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
        notebook.serialize(&mut serde_json::Serializer::with_formatter(
            &mut *output,
            formatter,
        ))?;
        output.write_all(b"\n")?;
        Ok(())
    })
}

pub(crate) fn check_expected_mtime(path: &Path, expected: Option<&Value>) -> Result<()> {
    check_mtime(path, expected_mtime(expected)?)
}

pub(crate) fn write_representation_atomic(
    path: &Path,
    content: &Value,
    expected: Option<&Value>,
) -> Result<Value> {
    write_file_atomic(path, expected, expected.is_none(), |output| {
        if let Some(notebook) = content.get("notebook") {
            let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
            notebook.serialize(&mut serde_json::Serializer::with_formatter(
                &mut *output,
                formatter,
            ))?;
            output.write_all(b"\n")?;
        } else {
            output.write_all(
                content["content"]
                    .as_str()
                    .context("Jupytext returned no file content")?
                    .as_bytes(),
            )?;
        }
        Ok(())
    })
}

pub(crate) fn recover_markdown(
    path: &Path,
    notebook: &Value,
    source_digest: &str,
) -> Result<PathBuf> {
    checked_digest(source_digest)?;
    let target = write_target(path)?;
    let before = file_mtime(&target)?.context("notebook disappeared during recovery")?;
    let source = fs::read(&target)
        .with_context(|| format!("could not read notebook {}", target.display()))?;
    let after = file_mtime(&target)?.context("notebook disappeared during recovery")?;
    if before != after {
        bail!("notebook changed on disk during recovery");
    }
    if !source_digest.eq_ignore_ascii_case(&file_digest_bytes(&source)) {
        bail!("notebook source digest does not match");
    }
    let content = std::str::from_utf8(&source).context("Jupytext source is not UTF-8")?;
    let filename = target
        .file_name()
        .context("notebook path has no filename")?
        .to_string_lossy();
    let backup = target.with_file_name(format!("{filename}.recovery-{source_digest}.bak"));
    match fs::symlink_metadata(&backup) {
        Ok(metadata) if metadata.file_type().is_file() => {
            if fs::read(&backup)? != source {
                bail!("recovery backup already exists with different contents");
            }
        }
        Ok(_) => bail!("recovery backup is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_representation_atomic(&backup, &json!({"content": content}), None)?;
            if !fs::symlink_metadata(&backup)?.file_type().is_file() {
                bail!("created recovery backup is not a regular file");
            }
            if fs::read(&backup)? != source {
                bail!("recovery backup contents do not match source");
            }
        }
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", backup.display()));
        }
    }
    File::open(&backup)
        .with_context(|| format!("could not open {} for fsync", backup.display()))?
        .sync_all()
        .with_context(|| format!("could not fsync {}", backup.display()))?;
    File::open(
        backup
            .parent()
            .context("backup path has no parent directory")?,
    )?
    .sync_all()
    .context("could not fsync recovery backup directory")?;
    write_atomic(
        path,
        notebook,
        Some(&json!({"mtime": after, "digest": source_digest})),
    )?;
    Ok(backup)
}

fn file_digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_file_atomic(
    path: &Path,
    expected: Option<&Value>,
    create_new: bool,
    write: impl FnOnce(&mut BufWriter<&mut File>) -> Result<()>,
) -> Result<Value> {
    let target = write_target(path)?;
    let expected_digest = expected
        .and_then(|value| value.get("digest"))
        .map(|value| {
            value
                .as_str()
                .context("expected digest must be a string")
                .and_then(checked_digest)
                .map(str::to_owned)
        })
        .transpose()?;
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
        write(&mut output).context("could not serialize notebook representation")?;
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

    let mtime = file_mtime(temporary.path())?.context("temporary notebook disappeared")?;
    let mut hash = Sha256::new();
    let mut file = File::open(temporary.path())?;
    let mut buffer = [0; 32768];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let digest = format!("{:x}", hash.finalize());

    // The conversion can take long enough for another process to modify the
    // file. Check once more immediately before the replacement.
    check_mtime(&target, expected)?;
    if let Some(expected_digest) = expected_digest {
        if !expected_digest.eq_ignore_ascii_case(&file_digest_bytes(&fs::read(&target)?)) {
            bail!("notebook changed on disk since it was opened");
        }
    }
    if create_new {
        temporary.persist_noclobber(&target)
    } else {
        temporary.persist(&target)
    }
    .map_err(|error| error.error)
    .with_context(|| format!("could not atomically replace {}", target.display()))?;
    File::open(parent)
        .with_context(|| format!("could not open {} for fsync", parent.display()))?
        .sync_all()
        .with_context(|| format!("could not fsync {}", parent.display()))?;

    Ok(json!({"mtime": mtime, "digest": digest}))
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
        assert_eq!(
            result["digest"],
            format!("{:x}", Sha256::digest(fs::read(&path)?))
        );

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

    #[test]
    fn markdown_recovery_preserves_source_and_guards_replacement() -> Result<()> {
        let directory = tempdir()?;
        let path = directory.path().join("notebook.ipynb");
        let source = b"# notebook\n\ntext\n";
        fs::write(&path, source)?;
        let digest = format!("{:x}", Sha256::digest(source));
        let mtime = file_mtime(&path)?.context("missing source mtime")?;
        let notebook = json!({"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 5});

        let mut wrong = digest.clone();
        wrong.replace_range(0..1, if &wrong[..1] == "0" { "1" } else { "0" });
        assert!(recover_markdown(&path, &notebook, &wrong).is_err());
        assert_eq!(file_mtime(&path)?, Some(mtime));
        assert_eq!(fs::read(&path)?, source);

        let backup = recover_markdown(&path, &notebook, &digest)?;
        assert_eq!(fs::read(&backup)?, source);
        assert_eq!(read_json(&path)?, notebook);

        fs::write(&path, source)?;
        let reused = recover_markdown(&path, &notebook, &digest)?;
        assert_eq!(reused, backup);
        assert_eq!(fs::read(&backup)?, source);

        fs::write(&path, source)?;
        fs::write(&backup, b"different")?;
        assert!(recover_markdown(&path, &notebook, &digest).is_err());
        assert_eq!(fs::read(&path)?, source);

        #[cfg(unix)]
        {
            fs::remove_file(&backup)?;
            std::os::unix::fs::symlink(&path, &backup)?;
            assert!(recover_markdown(&path, &notebook, &digest).is_err());
            assert!(fs::symlink_metadata(&backup)?.file_type().is_symlink());
            assert_eq!(fs::read(&path)?, source);
        }

        let unchanged_mtime = fs::metadata(&path)?.modified()?;
        let error = write_file_atomic(
            &path,
            Some(&json!({"mtime": file_mtime(&path)?.unwrap(), "digest": digest})),
            false,
            |output| {
                fs::write(&path, b"intervening same-mtime edit")?;
                File::open(&path)?
                    .set_times(std::fs::FileTimes::new().set_modified(unchanged_mtime))?;
                output.write_all(b"replacement")?;
                Ok(())
            },
        )
        .expect_err("digest guard must reject a same-mtime edit");
        assert!(error.to_string().contains("changed on disk"));
        assert_eq!(fs::read(&path)?, b"intervening same-mtime edit");
        Ok(())
    }
}
