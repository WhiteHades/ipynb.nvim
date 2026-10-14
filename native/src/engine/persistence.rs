use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

use super::{Engine, matching::positioned_sources, point, text};
use crate::notebook;

pub(super) fn imported_status(count: &Value) -> &'static str {
    if count.as_i64().is_some_and(|value| value != 0)
        || count.as_u64().is_some_and(|value| value != 0)
    {
        "done"
    } else {
        "new"
    }
}

impl Engine {
    pub(super) fn path(params: &Value) -> Result<&Path> {
        Ok(Path::new(
            params["path"]
                .as_str()
                .context("Notebook path is required")?,
        ))
    }

    fn read_compatible_notebook(&mut self, path: &Path) -> Result<Value> {
        let notebook = notebook::read_json(path)?;
        if notebook["nbformat"] == 4 {
            return Ok(notebook);
        }
        let mut converted = self.converter.read(path, Path::new(""))?;
        Ok(converted["notebook"].take())
    }

    pub(super) fn read(&mut self, params: &Value) -> Result<Value> {
        let path = Path::new(params["path"].as_str().context("Path required")?);
        let before = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut result = self
            .converter
            .read(path, Path::new(params["template"].as_str().unwrap_or("")))?;
        let after = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        self.read_cache = before
            .filter(|_| before == after)
            .map(|mtime| (path.to_owned(), mtime, result["notebook"].take()));
        Ok(json!({"text":result["text"], "metadata":result["metadata"]}))
    }

    pub(super) fn write(&mut self, params: &Value) -> Result<Value> {
        let path = Self::path(params)?;
        let existing = if path.is_file() && std::fs::metadata(path)?.len() > 0 {
            Some(notebook::read_json(path)?)
        } else {
            None
        };
        let identities: Vec<Value> = existing
            .as_ref()
            .and_then(|nb| nb["cells"].as_array())
            .into_iter()
            .flatten()
            .filter(|cell| cell["cell_type"] == "code")
            .map(|cell| {
                Value::Object(
                    cell.as_object()
                        .unwrap()
                        .iter()
                        .filter(|(key, _)| !matches!(key.as_str(), "outputs" | "execution_count"))
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                )
            })
            .collect();
        let buffer = text(&params["text"]);
        let mut notebook = self.converter.convert(&buffer, existing)?;
        self.preserve_cell_identity(
            &mut notebook,
            &identities,
            params["buf"].as_u64().unwrap_or(0),
            &params["normalized"],
        )?;
        self.merge_outputs(
            &mut notebook,
            params["buf"].as_u64().unwrap_or(0),
            None,
            &params["normalized"],
            Some(&buffer),
        )?;
        notebook::split_lines(&mut notebook);
        notebook::write_atomic(
            path,
            &notebook,
            params.get("mtime").filter(|v| !v.is_null()),
        )
    }

    pub(super) fn export(&mut self, params: &Value) -> Result<()> {
        let path = Self::path(params)?;
        let before = notebook::file_mtime(path)?.context("Notebook file is missing")?;
        let mut notebook = self.read_compatible_notebook(path)?;
        if notebook::file_mtime(path)? != Some(before) {
            bail!("notebook changed on disk while it was read");
        }
        let kernel = self.selected_kernel(params)?;
        if !self
            .cells
            .iter()
            .any(|cell| cell.buf == params["buf"].as_u64().unwrap_or(0) && cell.kernel == kernel)
        {
            return Ok(());
        }
        let buffer = params["lines"]
            .as_array()
            .map(|lines| lines.iter().map(text).collect::<Vec<_>>().join("\n"));
        let changed = self.merge_outputs(
            &mut notebook,
            params["buf"].as_u64().unwrap_or(0),
            Some(&kernel),
            &params["normalized"],
            buffer.as_deref(),
        )?;
        let overwrite = params["overwrite"].as_bool().unwrap_or(true);
        if changed || !overwrite {
            let dest = if overwrite {
                path.to_owned()
            } else {
                path.with_file_name(format!(
                    "copy-of-{}",
                    path.file_name().unwrap().to_string_lossy()
                ))
            };
            notebook::split_lines(&mut notebook);
            let expected = json!(before);
            notebook::write_atomic(&dest, &notebook, overwrite.then_some(&expected))?;
        }
        Ok(())
    }

    pub(super) fn import(&mut self, params: &Value) -> Result<()> {
        let path = Self::path(params)?;
        let notebook = match self.read_cache.take() {
            Some((cached_path, modified, notebook))
                if cached_path == path
                    && std::fs::metadata(path).and_then(|m| m.modified()).ok()
                        == Some(modified) =>
            {
                notebook
            }
            _ => self.read_compatible_notebook(path)?,
        };
        let buf = params["buf"].as_u64().unwrap_or(0);
        let kernel = if self.attached.get(&buf).is_some_and(|ids| !ids.is_empty()) {
            self.selected_kernel(params)?
        } else {
            let id = format!("saved:{buf}");
            self.languages.insert(
                id.clone(),
                notebook["metadata"]["language_info"]["name"]
                    .as_str()
                    .unwrap_or("python")
                    .to_owned(),
            );
            id
        };
        let lines: Vec<String> = params["lines"]
            .as_array()
            .context("Buffer lines are required")?
            .iter()
            .map(text)
            .collect();
        let spans = params["spans"].as_array();
        let (sources, positions) = if let Some(spans) = spans {
            let starts: Vec<_> = spans
                .iter()
                .enumerate()
                .map(|(index, span)| (index as u64, point(&span["begin"])))
                .collect();
            positioned_sources(&mut self.converter, &lines.join("\n"), &starts)?
        } else {
            (Vec::new(), HashMap::new())
        };
        let mut offset = 0;
        let mut ordinal = 0;
        for nb in notebook["cells"]
            .as_array()
            .context("Notebook cells missing")?
        {
            if nb["cell_type"] != "code" {
                continue;
            }
            let source = text(&nb["source"]);
            let code: Vec<&str> = source.split('\n').collect();
            let start = if let Some(spans) = spans {
                spans
                    .iter()
                    .enumerate()
                    .find(|(index, _)| {
                        positions.get(&(*index as u64)) == Some(&ordinal)
                            && sources.get(ordinal) == Some(&source)
                    })
                    .map(|(_, span)| point(&span["begin"])[0])
            } else {
                (offset..lines.len()).find(|&i| {
                    i + code.len() <= lines.len()
                        && lines[i..i + code.len()]
                            .iter()
                            .zip(&code)
                            .all(|(a, b)| a == b)
                })
            };
            ordinal += 1;
            let Some(start) = start else {
                continue;
            };
            let end = start + code.len() - 1;
            if end >= lines.len() {
                bail!("Saved cell extends outside the notebook buffer");
            }
            offset = end + 1;
            let id = self.new_cell(
                buf,
                kernel.clone(),
                [start, 0],
                [end, lines[end].len()],
                source,
            )?;
            let cell = self.cells.iter_mut().find(|c| c.id == id).unwrap();
            cell.outputs = nb["outputs"].as_array().cloned().unwrap_or_default();
            cell.count = nb["execution_count"].clone();
            cell.status = imported_status(&cell.count);
            cell.old = true;
            cell.success = !cell.outputs.iter().any(|o| o["output_type"] == "error");
        }
        self.dirty.insert(buf);
        Ok(())
    }
}
