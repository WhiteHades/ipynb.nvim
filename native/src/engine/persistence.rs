use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

use super::{Engine, ReadCache, matching::positioned_sources, point, text};
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

fn initial_positions(
    notebook: &Value,
    lines: &[String],
    spans: &[Value],
) -> Option<(Vec<String>, HashMap<u64, usize>)> {
    let sources: Vec<_> = notebook["cells"]
        .as_array()?
        .iter()
        .filter(|cell| cell["cell_type"] == "code")
        .map(|cell| text(&cell["source"]))
        .collect();
    let advance = |protected: &mut bool, gap: &[String]| -> Option<()> {
        for line in gap {
            match line.as_str() {
                "<!-- #region -->" if !*protected => *protected = true,
                "<!-- #endregion -->" if *protected => *protected = false,
                "<!-- #region -->" | "<!-- #endregion -->" => return None,
                _ => {}
            }
        }
        Some(())
    };
    let mut protected = false;
    let mut cursor = 0;
    let mut positions = HashMap::new();
    for (index, span) in spans.iter().enumerate() {
        let first = point(&span["begin"]);
        let last = point(&span["end"]);
        advance(&mut protected, lines.get(cursor..first[0])?)?;
        let body = lines.get(first[0]..=last[0])?;
        if body
            .iter()
            .any(|line| matches!(line.as_str(), "<!-- #region -->" | "<!-- #endregion -->"))
        {
            return None;
        }
        cursor = last[0].checked_add(1)?;
        if protected {
            continue;
        }
        let ordinal = positions.len();
        let source = sources.get(ordinal)?;
        if first[1] != 0 || last[1] != lines.get(last[0])?.len() || body.join("\n") != *source {
            return None;
        }
        positions.insert(index as u64, ordinal);
    }
    advance(&mut protected, lines.get(cursor..)?)?;
    if protected || positions.len() != sources.len() {
        return None;
    }
    Some((sources, positions))
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
        if let Ok(notebook) = notebook::read_json(path)
            && notebook["nbformat"] == 4
        {
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
        if before != after {
            bail!("Notebook changed on disk while it was opened");
        }
        if let Some(digest) = result["recovery_digest"].as_str()
            && params["readonly"] != true
        {
            if params["recovery_attempted"] == true {
                bail!("Notebook changed again during recovery; reopen it");
            }
            let mut notebook = result["notebook"].clone();
            notebook::split_lines(&mut notebook);
            let backup = notebook::recover_markdown(path, &notebook, digest)?;
            let mut retry = params.clone();
            retry["recovery_attempted"] = json!(true);
            let mut recovered = self.read(&retry)?;
            recovered["recovery_backup"] = json!(backup);
            return Ok(recovered);
        }
        self.remember_pairs(path, &result)?;
        self.read_cache = before
            .filter(|_| before == after)
            .map(|modified| ReadCache {
                path: path.to_owned(),
                modified,
                text_digest: Sha256::digest(result["text"].as_str().unwrap_or("").as_bytes())
                    .into(),
                notebook: result["notebook"].take(),
            });
        Ok(
            json!({"text":result["text"], "metadata":result["metadata"], "paired_changed":result["divergent"],
                "recovery_pending":result["recovery_digest"].is_string()}),
        )
    }

    pub(super) fn write(&mut self, params: &Value) -> Result<Value> {
        self.write_pairs(params)
    }

    pub(super) fn current_notebook(&mut self, params: &Value) -> Result<Value> {
        let path = Self::path(params)?;
        let existing = if path.is_file() && std::fs::metadata(path)?.len() > 0 {
            Some(self.read_compatible_notebook(path)?)
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
        let mut notebook = self.converter.convert_at(&buffer, existing, Some(path))?;
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
            Some(path),
        )?;
        Ok(notebook)
    }

    pub(super) fn export(&mut self, params: &Value) -> Result<Value> {
        let path = Self::path(params)?;
        let overwrite = params["overwrite"].as_bool().unwrap_or(true);
        if overwrite {
            notebook::check_expected_mtime(path, params.get("mtime"))?;
            self.check_pair_file(path)?;
        }
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
            return Ok(json!({"mtime":before}));
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
            Some(path),
        )?;
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
            if overwrite {
                self.check_pair_file(path)?;
            }
            let result = notebook::write_atomic(&dest, &notebook, overwrite.then_some(&expected))?;
            if overwrite {
                self.refresh_pair_file(path, &result)?;
            }
            return Ok(result);
        }
        Ok(json!({"mtime":before}))
    }

    pub(super) fn import(&mut self, params: &Value) -> Result<Vec<Value>> {
        let path = Self::path(params)?;
        let (notebook, read_digest) = if params["notebook"].is_object() {
            (params["notebook"].clone(), None)
        } else {
            match self.read_cache.take() {
                Some(cached)
                    if cached.path == path
                        && std::fs::metadata(path).and_then(|m| m.modified()).ok()
                            == Some(cached.modified) =>
                {
                    (cached.notebook, Some(cached.text_digest))
                }
                _ => (self.read_compatible_notebook(path)?, None),
            }
        };
        let buf = params["buf"].as_u64().unwrap_or(0);
        let kernel = if self
            .attached
            .get(&buf)
            .is_some_and(|ids| !ids.is_empty() && (params["preview"] != true || ids.len() == 1))
        {
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
            let buffer = lines.join("\n");
            // Only an untouched read can reuse its verified cell ordering.
            let initial = (params["preview"] == true
                && read_digest == Some(Sha256::digest(buffer.as_bytes()).into()))
            .then(|| initial_positions(&notebook, &lines, spans))
            .flatten();
            match initial {
                Some(positions) => positions,
                None => positioned_sources(&mut self.converter, &buffer, &starts, Some(path))?,
            }
        } else {
            (Vec::new(), HashMap::new())
        };
        let mut span_rows = vec![None; sources.len()];
        for (index, span) in spans.into_iter().flatten().enumerate() {
            if let Some(row) = positions
                .get(&(index as u64))
                .and_then(|ordinal| span_rows.get_mut(*ordinal))
            {
                row.get_or_insert(point(&span["begin"])[0]);
            }
        }
        let mut offset = 0;
        let mut ordinal = 0;
        if params["reset"] == true {
            if self
                .cells
                .iter()
                .any(|cell| cell.buf == buf && matches!(cell.status, "running" | "hold"))
            {
                bail!("Wait for execution to finish before replacing output previews");
            }
            self.cells.retain(|cell| cell.buf != buf);
            self.prune_requests();
            self.read_cache = None;
        }
        for nb in notebook["cells"]
            .as_array()
            .context("Notebook cells missing")?
        {
            if nb["cell_type"] != "code" {
                continue;
            }
            let source = text(&nb["source"]);
            let code: Vec<&str> = source.split('\n').collect();
            let start = if spans.is_some() {
                span_rows
                    .get(ordinal)
                    .copied()
                    .flatten()
                    .filter(|_| sources.get(ordinal) == Some(&source))
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
        Ok(spans
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(index, _)| positions.contains_key(&(*index as u64)))
            .map(|(_, span)| span.clone())
            .collect())
    }
}
