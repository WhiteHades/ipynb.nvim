//! Jupytext plans conversions; the engine guards and commits their files.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::Engine;
use crate::notebook::{self, Mtime};

#[derive(Clone, PartialEq, Eq)]
struct FileState {
    mtime: Option<Mtime>,
    digest: Option<String>,
}

impl FileState {
    fn read(path: &Path) -> Result<Self> {
        let mtime = notebook::file_mtime(path)?;
        let digest = if mtime.is_some() {
            let mut file = std::fs::File::open(path)?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 32768];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
            Some(format!("{:x}", hash.finalize()))
        } else {
            None
        };
        if notebook::file_mtime(path)? != mtime {
            bail!("{} changed while it was read", path.display());
        }
        Ok(Self { mtime, digest })
    }

    fn check(&self, path: &Path) -> Result<()> {
        if &Self::read(path)? != self {
            bail!(
                "{} changed on disk; no further files were written",
                path.display()
            );
        }
        Ok(())
    }

    fn written(result: &Value) -> Result<Self> {
        Ok(Self {
            mtime: Some(serde_json::from_value(result["mtime"].clone())?),
            digest: Some(
                result["digest"]
                    .as_str()
                    .context("Written digest is missing")?
                    .to_owned(),
            ),
        })
    }
}

pub(super) struct PairState {
    files: BTreeMap<PathBuf, FileState>,
    divergent: Vec<PathBuf>,
}

fn pair_files(path: &Path, info: &Value) -> Result<BTreeMap<PathBuf, FileState>> {
    let mut files = BTreeMap::new();
    files.insert(path.to_owned(), FileState::read(path)?);
    for pair in info["pairs"].as_array().into_iter().flatten() {
        let path = PathBuf::from(
            pair["path"]
                .as_str()
                .context("Jupytext pair path is missing")?,
        );
        if !files.contains_key(&path) {
            files.insert(path.clone(), FileState::read(&path)?);
        }
    }
    Ok(files)
}

impl Engine {
    pub(super) fn check_pair_file(&self, path: &Path) -> Result<()> {
        if let Some(previous) = self
            .paired
            .get(path)
            .and_then(|state| state.files.get(path))
        {
            previous.check(path)?;
        }
        Ok(())
    }

    pub(super) fn refresh_pair_file(&mut self, path: &Path, result: &Value) -> Result<()> {
        if let Some(state) = self.paired.get_mut(path) {
            state
                .files
                .insert(path.to_owned(), FileState::written(result)?);
        }
        Ok(())
    }

    pub(super) fn remember_pairs(&mut self, path: &Path, info: &Value) -> Result<()> {
        let files = pair_files(path, info)?;
        for pair in info["pairs"]
            .as_array()
            .context("Jupytext pairs are missing")?
        {
            let name = Path::new(
                pair["path"]
                    .as_str()
                    .context("Jupytext pair path is missing")?,
            );
            let digest = pair
                .get("digest")
                .context("Jupytext read digest is missing")?;
            if files[name].digest.as_deref() != digest.as_str() {
                bail!(
                    "{} changed on disk while it was opened; reopen the notebook",
                    name.display()
                );
            }
        }
        self.remember_pair_files(path, info, files);
        Ok(())
    }

    fn remember_pair_files(
        &mut self,
        path: &Path,
        info: &Value,
        files: BTreeMap<PathBuf, FileState>,
    ) {
        self.paired.insert(
            path.to_owned(),
            PairState {
                files,
                divergent: info["divergent"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect(),
            },
        );
    }

    fn changed_pairs(&self, path: &Path, files: &BTreeMap<PathBuf, FileState>) -> Vec<PathBuf> {
        let previous = self.paired.get(path);
        files
            .iter()
            .filter(|(file, current)| {
                if let Some(previous) = previous {
                    previous.files.get(*file) != Some(*current) || previous.divergent.contains(file)
                } else {
                    file.as_path() != path && current.mtime.is_some()
                }
            })
            .map(|(file, _)| file.clone())
            .collect()
    }

    pub(super) fn write_pairs(&mut self, params: &Value) -> Result<Value> {
        let mut params = params.clone();
        params["action"] = json!("save");
        self.jupytext(&params)
    }

    pub(super) fn jupytext(&mut self, params: &Value) -> Result<Value> {
        let path = Self::path(params)?.to_owned();
        let action = params["action"].as_str().unwrap_or("info");
        let buf = params["buf"].as_u64().unwrap_or(0);
        if matches!(action, "sync" | "pair" | "unpair" | "pipe")
            && self
                .cells
                .iter()
                .any(|cell| cell.buf == buf && matches!(cell.status, "running" | "hold"))
        {
            bail!("Wait for execution to finish before changing notebook inputs");
        }
        if !matches!(action, "info" | "sync") {
            notebook::check_expected_mtime(&path, params.get("mtime"))?;
        }
        let before = FileState::read(&path)?;
        let mut observed = BTreeMap::new();
        if let Some(previous) = self.paired.get(&path) {
            for file in previous.files.keys().filter(|file| **file != path) {
                observed.insert(file.clone(), FileState::read(file)?);
            }
        }
        observed.insert(path.clone(), before.clone());
        if action == "sync"
            && let Some(source) = params["source"]
                .as_str()
                .filter(|source| *source != "buffer")
        {
            let source = PathBuf::from(source);
            observed.insert(source.clone(), FileState::read(&source)?);
        }
        let current = self.current_notebook(params)?;
        before.check(&path)?;
        let changed = self.changed_pairs(&path, &observed);
        if action == "info" {
            let mut info = self
                .converter
                .call(&json!({"op":"jupytext_info", "path":path, "notebook":current}))?;
            info["changed"] = json!(changed);
            return Ok(info);
        }
        let explicit_source = action == "sync" && params["source"].as_str().is_some();
        if action != "export" && !explicit_source && !changed.is_empty() {
            bail!(
                "Paired files changed or differ: {}. Use :IpynbJupytext sync to choose the source; nothing was written",
                changed
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if action == "export" {
            let target = PathBuf::from(
                params["target"]
                    .as_str()
                    .context("Export target required")?,
            );
            if target == path
                || std::fs::canonicalize(&target).ok().is_some_and(|target| {
                    std::fs::canonicalize(&path).ok().as_ref() == Some(&target)
                })
            {
                bail!("Export cannot replace the open notebook; use :w");
            }
            observed.insert(target.clone(), FileState::read(&target)?);
        }
        let mut request = params.clone();
        request["op"] = json!("jupytext_action");
        request["action"] = json!(if action == "save" { "sync" } else { action });
        request["notebook"] = current;
        if request["source"] == "buffer" {
            request.as_object_mut().unwrap().remove("source");
        }
        let mut plan = self.converter.call(&request)?;
        if matches!(action, "pipe" | "check") {
            return Ok(plan);
        }
        let writes = plan["writes"]
            .as_array_mut()
            .context("Jupytext returned no write plan")?;
        for entry in writes.iter_mut() {
            let target = PathBuf::from(
                entry["path"]
                    .as_str()
                    .context("Jupytext destination required")?,
            );
            if !observed.contains_key(&target) {
                observed.insert(target.clone(), FileState::read(&target)?);
            }
            let previous = &observed[&target];
            let owned = action != "export"
                && (target == path
                    || self
                        .paired
                        .get(&path)
                        .is_some_and(|state| state.files.contains_key(&target)));
            if previous.mtime.is_some() && !owned && params["overwrite"] != true {
                bail!(
                    "{} already exists; choose a different destination or explicitly approve replacement",
                    target.display()
                );
            }
            if let Some(notebook) = entry.get_mut("notebook") {
                notebook::split_lines(notebook);
            }
        }
        // Render and check every file before changing the first destination.
        for (file, state) in &observed {
            state.check(file)?;
        }
        let mut paths = Vec::new();
        let mut mtime = Value::Null;
        for entry in writes.iter() {
            let target = Path::new(entry["path"].as_str().unwrap());
            let previous = &observed[target];
            let result = (|| -> Result<Value> {
                previous.check(target)?;
                let expected = previous.mtime.map(|mtime| json!(mtime));
                notebook::write_representation_atomic(target, entry, expected.as_ref())
            })()
            .with_context(|| format!("Jupytext save failed; files already written: {:?}", paths))?;
            paths.push(target.to_owned());
            observed.insert(target.to_owned(), FileState::written(&result)?);
            if target == path {
                mtime = result["mtime"].clone();
            }
        }
        if action != "export" {
            if action == "save" {
                plan["text"] = params["text"].clone();
            }
            observed.retain(|file, _| {
                file == &path
                    || plan["pairs"].as_array().is_some_and(|pairs| {
                        pairs.iter().any(|pair| {
                            pair["path"]
                                .as_str()
                                .is_some_and(|name| Path::new(name) == file)
                        })
                    })
            });
            self.remember_pair_files(&path, &plan, observed);
            self.read_cache = None;
            if action != "save" {
                self.cells.retain(|cell| cell.buf != buf);
                self.prune_requests();
                self.dirty.insert(buf);
            }
        }
        Ok(json!({"text":plan["text"], "metadata":plan["metadata"], "mtime":mtime, "paths":paths}))
    }
}
