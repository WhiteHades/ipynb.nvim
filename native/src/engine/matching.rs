use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use super::{Cell, Engine, SourceComparison, text};
use crate::notebook::{self, Converter};

pub(super) fn comparison_sources(
    cells: &[&Cell],
    notebook_cells: &[Value],
    languages: &HashMap<String, String>,
) -> SourceComparison {
    let mut comparison = BTreeMap::<String, Vec<String>>::new();
    let notebook_sources: Vec<String> = notebook_cells
        .iter()
        .filter(|cell| cell["cell_type"] == "code")
        .map(|cell| text(&cell["source"]))
        .collect();
    for cell in cells {
        let language = languages
            .get(&cell.kernel)
            .cloned()
            .unwrap_or_else(|| "python".into());
        comparison.entry(language).or_default();
    }
    for sources in comparison.values_mut() {
        sources.extend(notebook_sources.iter().cloned());
        sources.extend(cells.iter().map(|cell| cell.source.clone()));
        sources.sort_unstable();
        sources.dedup();
    }
    SourceComparison(comparison)
}

fn same_source(left: &str, right: &str, language: &str, normalized: &Value) -> Option<bool> {
    if left == right {
        return Some(true);
    }
    let sources = normalized.get(language)?;
    Some(sources.get(left)? == sources.get(right)?)
}

pub(super) fn positioned_sources(
    converter: &mut Converter,
    buffer: &str,
    starts: &[(u64, [usize; 2])],
    path: Option<&Path>,
) -> Result<(Vec<String>, HashMap<u64, usize>)> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut marked = buffer.to_owned();
    let lines: Vec<&str> = buffer.split('\n').collect();
    let mut insertions = Vec::new();
    for (id, begin) in starts {
        let line = lines
            .get(begin[0])
            .context("Output position is outside the buffer")?;
        if !line.is_char_boundary(begin[1]) {
            bail!("Output position is outside a UTF-8 boundary");
        }
        let offset = lines[..begin[0]]
            .iter()
            .map(|line| line.len() + 1)
            .sum::<usize>()
            + begin[1];
        insertions.push((offset, *id, format!("__ipynb_{nonce}_{id}__")));
    }
    insertions.sort_by_key(|(offset, _, _)| std::cmp::Reverse(*offset));
    for (offset, _, marker) in &insertions {
        marked.insert_str(*offset, marker);
    }
    let marker_prefix = format!("__ipynb_{nonce}_");
    let marker_ids: HashMap<_, _> = insertions
        .iter()
        .map(|(_, id, marker)| (marker.as_str(), *id))
        .collect();
    // Let Jupytext locate the spans; source searches also match prose and duplicate cells.
    let converted = converter.convert_at(&marked, None, path)?;
    let mut sources = Vec::new();
    let mut positions = HashMap::new();
    for cell in converted["cells"]
        .as_array()
        .context("Notebook cells must be an array")?
    {
        if cell["cell_type"] != "code" {
            continue;
        }
        let source = text(&cell["source"]);
        let mut clean = String::with_capacity(source.len());
        let mut cursor = 0;
        for (start, _) in source.match_indices(&marker_prefix) {
            let digits = start + marker_prefix.len();
            let Some(suffix) = source[digits..].find("__") else {
                continue;
            };
            let end = digits + suffix + 2;
            if let Some(id) = marker_ids.get(&source[start..end]) {
                if positions
                    .insert(*id, sources.len())
                    .is_some_and(|previous| previous != sources.len())
                {
                    bail!("Ambiguous notebook output position; file was not changed");
                }
                clean.push_str(&source[cursor..start]);
                cursor = end;
            }
        }
        clean.push_str(&source[cursor..]);
        sources.push(clean);
    }
    Ok((sources, positions))
}

impl Engine {
    pub(super) fn merge_outputs(
        &mut self,
        notebook: &mut Value,
        buf: u64,
        kernel: Option<&str>,
        normalized: &Value,
        buffer: Option<&str>,
        path: Option<&Path>,
    ) -> Result<bool> {
        let mut cells: Vec<_> = self
            .cells
            .iter()
            .filter(|c| c.buf == buf && kernel.is_none_or(|k| c.kernel == k))
            .collect();
        cells.sort_by_key(|c| c.begin);
        if cells.is_empty() {
            return Ok(false);
        }
        let nb_cells = notebook["cells"]
            .as_array_mut()
            .context("Notebook cells must be an array")?;
        let (buffer_sources, positions) = match buffer {
            Some(buffer) => positioned_sources(
                &mut self.converter,
                buffer,
                &cells
                    .iter()
                    .map(|cell| (cell.id, cell.begin))
                    .collect::<Vec<_>>(),
                path,
            )?,
            None => (Vec::new(), HashMap::new()),
        };
        let comparison = comparison_sources(&cells, nb_cells, &self.languages);
        let code_cells: Vec<_> = nb_cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| cell["cell_type"] == "code")
            .map(|(index, cell)| (index, text(&cell["source"])))
            .collect();
        let aligned = buffer_sources.len() == code_cells.len()
            && buffer_sources
                .iter()
                .zip(&code_cells)
                .all(|(current, (_, saved))| current == saved);
        let mut assigned = BTreeSet::new();
        let mut matches = Vec::new();
        for cell in &cells {
            if buffer.is_some() && !positions.contains_key(&cell.id) {
                bail!(
                    "No notebook cell at output line {}; file was not changed",
                    cell.begin[0] + 1
                );
            }
            let language = self
                .languages
                .get(&cell.kernel)
                .map(String::as_str)
                .unwrap_or("python");
            let equal = |left: &str, right: &str| -> Result<bool> {
                same_source(left, right, language, normalized)
                    .ok_or_else(|| SourceComparison(comparison.0.clone()).into())
            };
            let positional = positions.get(&cell.id).filter(|_| aligned).copied();
            let exact =
                positional.is_none() && code_cells.iter().any(|(_, source)| source == &cell.source);
            let mut candidates = Vec::new();
            for (ordinal, (index, source)) in code_cells.iter().enumerate() {
                if positional.is_some_and(|position| position != ordinal) {
                    continue;
                }
                if exact && source != &cell.source {
                    continue;
                }
                if equal(source, &cell.source)? {
                    candidates.push(*index);
                }
            }
            let index = match candidates.as_slice() {
                [index] if assigned.insert(*index) => *index,
                [] => bail!(
                    "No notebook cell matches output at line {}; file was not changed",
                    cell.begin[0] + 1
                ),
                _ => bail!(
                    "Ambiguous notebook cell for output at line {}; file was not changed",
                    cell.begin[0] + 1
                ),
            };
            matches.push(index);
        }
        let mut changed = false;
        for (cell, index) in cells.into_iter().zip(matches) {
            let nb = &mut nb_cells[index];
            let mut outputs = cell.outputs.clone();
            for output in &mut outputs {
                if let Some(obj) = output.as_object_mut() {
                    obj.remove("transient");
                }
                if output["output_type"] == "execute_result" && output["execution_count"].is_null()
                {
                    output["execution_count"] = cell.count.clone();
                }
                notebook::split_output(output);
            }
            changed |= nb["outputs"] != json!(outputs) || nb["execution_count"] != cell.count;
            nb["outputs"] = json!(outputs);
            nb["execution_count"] = cell.count.clone();
        }
        Ok(changed)
    }

    pub(super) fn preserve_cell_identity(
        &self,
        notebook: &mut Value,
        originals: &[Value],
        buf: u64,
        normalized: &Value,
    ) -> Result<()> {
        let cells: Vec<_> = self.cells.iter().filter(|cell| cell.buf == buf).collect();
        let nb_cells = notebook["cells"]
            .as_array_mut()
            .context("Notebook cells must be an array")?;
        let original_ids: BTreeSet<_> = originals
            .iter()
            .filter_map(|cell| cell["id"].as_str())
            .collect();
        let mut used: BTreeSet<String> = nb_cells
            .iter()
            .filter_map(|cell| cell["id"].as_str().map(str::to_owned))
            .collect();
        if cells.is_empty() || original_ids.iter().all(|id| used.contains(*id)) {
            return Ok(());
        }
        let unmatched_sources: Vec<_> = nb_cells
            .iter()
            .filter(|cell| {
                cell["cell_type"] == "code"
                    && !cell["id"]
                        .as_str()
                        .is_some_and(|id| original_ids.contains(id))
            })
            .map(|cell| text(&cell["source"]))
            .collect();
        let mut compared = originals.to_vec();
        compared.extend(
            nb_cells
                .iter()
                .map(|cell| json!({"cell_type":cell["cell_type"],"source":cell["source"]})),
        );
        let comparison = comparison_sources(&cells, &compared, &self.languages);
        for nb in nb_cells
            .iter_mut()
            .filter(|cell| cell["cell_type"] == "code")
        {
            if nb["id"]
                .as_str()
                .is_some_and(|id| original_ids.contains(id))
            {
                continue;
            }
            let source = text(&nb["source"]);
            // Jupytext's fallback cannot retain identity after both moving and editing comments.
            for cell in &cells {
                let language = self
                    .languages
                    .get(&cell.kernel)
                    .map(String::as_str)
                    .unwrap_or("python");
                let equal = |left: &str, right: &str| -> Result<bool> {
                    same_source(left, right, language, normalized)
                        .ok_or_else(|| SourceComparison(comparison.0.clone()).into())
                };
                if !equal(&source, &cell.source)? {
                    continue;
                }
                let mut candidates = Vec::new();
                for previous in originals {
                    if previous["id"].as_str().is_some_and(|id| !used.contains(id))
                        && equal(&source, &text(&previous["source"]))?
                    {
                        candidates.push(previous);
                    }
                }
                if candidates.len() > 1 {
                    bail!(
                        "Ambiguous notebook cell identity at output line {}; file was not changed",
                        cell.begin[0] + 1
                    );
                }
                if let Some(previous) = candidates.first() {
                    let mut possible = 0;
                    for current in &unmatched_sources {
                        if equal(current, &text(&previous["source"]))? {
                            possible += 1;
                        }
                    }
                    if possible > 1 {
                        bail!(
                            "Ambiguous notebook cell identity at output line {}; file was not changed",
                            cell.begin[0] + 1
                        );
                    }
                    nb["id"] = previous["id"].clone();
                    used.insert(text(&previous["id"]));
                    if let (Some(metadata), Some(saved)) = (
                        nb["metadata"].as_object_mut(),
                        previous["metadata"].as_object(),
                    ) {
                        for (key, value) in saved {
                            metadata.entry(key.clone()).or_insert_with(|| value.clone());
                        }
                    }
                    for (key, value) in previous
                        .as_object()
                        .context("Notebook cell must be an object")?
                    {
                        nb.as_object_mut()
                            .unwrap()
                            .entry(key.clone())
                            .or_insert_with(|| value.clone());
                    }
                }
                break;
            }
        }
        Ok(())
    }
}
