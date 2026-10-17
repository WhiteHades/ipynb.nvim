mod cells;
mod jupytext;
mod kernels;
mod matching;
mod outputs;
mod persistence;
mod rendering;
mod saved_state;

#[cfg(test)]
mod tests;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::kernel::Kernel;
use crate::notebook::Converter;
use cells::Cell;

#[derive(Debug)]
pub struct SourceComparison(pub BTreeMap<String, Vec<String>>);
impl std::fmt::Display for SourceComparison {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Source comparison is required")
    }
}
impl std::error::Error for SourceComparison {}

struct ReadCache {
    path: PathBuf,
    modified: std::time::SystemTime,
    notebook: Value,
    text_digest: [u8; 32],
}

pub struct Engine {
    python: String,
    converter: Converter,
    kernels: BTreeMap<String, Kernel>,
    attached: BTreeMap<u64, Vec<String>>,
    cells: Vec<Cell>,
    requests: HashMap<String, u64>,
    languages: HashMap<String, String>,
    kernel_names: HashMap<String, String>,
    kernel_details: BTreeMap<String, Value>,
    next_cell: u64,
    options: Value,
    assets: tempfile::TempDir,
    read_cache: Option<ReadCache>,
    paired: HashMap<PathBuf, jupytext::PairState>,
    dirty: BTreeSet<u64>,
    view_cache: HashMap<u64, Value>,
    events: Vec<Value>,
}

pub fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(lines) => lines.iter().map(text).collect(),
        _ => String::new(),
    }
}

fn point(value: &Value) -> [usize; 2] {
    [
        value[0].as_u64().unwrap_or(0) as usize,
        value[1].as_u64().unwrap_or(0) as usize,
    ]
}

impl Engine {
    pub fn new(python: String, converter: &Path) -> Result<Self> {
        Ok(Self {
            converter: Converter::new(&python, converter),
            python,
            kernels: BTreeMap::new(),
            attached: BTreeMap::new(),
            cells: vec![],
            requests: HashMap::new(),
            languages: HashMap::new(),
            kernel_names: HashMap::new(),
            kernel_details: BTreeMap::new(),
            next_cell: 1,
            options: json!({}),
            assets: tempfile::Builder::new().prefix("ipynb-native-").tempdir()?,
            dirty: BTreeSet::new(),
            view_cache: HashMap::new(),
            events: vec![],
            read_cache: None,
            paired: HashMap::new(),
        })
    }

    pub fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value> {
        self.sync_positions(params);
        let buf = params["buf"].as_u64().unwrap_or(0);
        match method {
            "warmup" => {
                self.converter.warmup()?;
                Ok(Value::Null)
            }
            "configure" => {
                self.options = params.clone();
                self.view_cache.clear();
                Ok(self.state())
            }
            "available" => {
                let specifications = self.kernel_specifications()?;
                let mut names: Vec<&String> = specifications.keys().collect();
                names.sort_unstable();
                Ok(json!(names))
            }
            "state" | "tick" => {
                self.poll()?;
                Ok(self.state())
            }
            "init" => self.init(params),
            "deinit" | "unload" => self.deinit(buf),
            "code_spans" => {
                let spans = params["spans"]
                    .as_array()
                    .context("Cell spans are required")?;
                let starts: Vec<_> = spans
                    .iter()
                    .enumerate()
                    .map(|(index, span)| (index as u64, point(&span["begin"])))
                    .collect();
                let positioned = matching::positioned_sources(
                    &mut self.converter,
                    &text(&params["text"]),
                    &starts,
                    params["path"].as_str().map(Path::new),
                )?;
                Ok(json!(
                    spans
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| positioned.positions.contains_key(&(*index as u64)))
                        .map(|(_, span)| span)
                        .collect::<Vec<_>>()
                ))
            }
            "execute" | "define" => {
                let kernel = self.selected_kernel(params)?;
                self.kernels
                    .get(&kernel)
                    .context("Kernel is stopped; initialize it again")?
                    .ensure_open()?;
                let id = self.new_cell(
                    buf,
                    kernel,
                    point(&params["begin"]),
                    point(&params["end"]),
                    text(&params["source"]),
                )?;
                if method == "execute" {
                    self.execute_cell(id)?;
                }
                Ok(json!(id))
            }
            "reevaluate" => {
                let ids: Vec<u64> = if let Some(id) = params["cell"].as_u64() {
                    vec![id]
                } else {
                    let mut cells: Vec<_> = self.cells.iter().filter(|c| c.buf == buf).collect();
                    cells.sort_by_key(|c| c.begin);
                    cells.iter().map(|c| c.id).collect()
                };
                for id in ids {
                    if self
                        .cells
                        .iter()
                        .any(|cell| cell.id == id && cell.kernel.starts_with("saved:"))
                    {
                        let kernel = self.selected_kernel(params)?;
                        if let Some(cell) = self.cells.iter_mut().find(|cell| cell.id == id) {
                            cell.kernel = kernel;
                        }
                    }
                    self.execute_cell(id)?;
                }
                Ok(Value::Null)
            }
            "delete" => {
                let id = params["cell"].as_u64();
                if self.cells.iter().any(|c| {
                    c.buf == buf && (id.is_none() || id == Some(c.id)) && c.status == "running"
                }) {
                    bail!("Interrupt running cells before deleting them");
                }
                self.cells
                    .retain(|c| c.buf != buf || id.is_some() && id != Some(c.id));
                self.prune_requests();
                self.dirty.insert(buf);
                Ok(Value::Null)
            }
            "interrupt" | "restart" => {
                let id = self.selected_kernel(params)?;
                let kernel = self.kernels.get_mut(&id).context("Kernel unavailable")?;
                if method == "interrupt" {
                    kernel.interrupt()?;
                } else {
                    kernel.restart()?;
                    let old_requests: std::collections::HashSet<u64> = self
                        .cells
                        .iter()
                        .filter(|c| c.kernel == id)
                        .map(|c| c.id)
                        .collect();
                    self.requests.retain(|_, cell| !old_requests.contains(cell));
                    for cell in self.cells.iter_mut().filter(|c| c.kernel == id) {
                        self.view_cache.remove(&cell.id);
                        cell.status = "done";
                        cell.old = true;
                        self.dirty.insert(cell.buf);
                    }
                    if params["clear"].as_bool().unwrap_or(false) {
                        self.cells.retain(|c| c.kernel != id);
                    }
                }
                Ok(Value::Null)
            }
            "stdin" => {
                let id = params["kernel"].as_str().context("Kernel required")?;
                self.kernels
                    .get_mut(id)
                    .context("Kernel unavailable")?
                    .stdin(&text(&params["value"]))?;
                Ok(Value::Null)
            }
            "read" => self.read(params),
            "write" => self.write(params),
            "jupytext" => self.jupytext(params),
            "preview" => {
                let mut params = params.clone();
                params["preview"] = json!(true);
                params["reset"] = json!(true);
                Ok(json!(self.import(&params)?))
            }
            "import" => {
                self.import(params)?;
                Ok(Value::Null)
            }
            "export" => self.export(params),
            "save" => {
                self.save(params)?;
                Ok(Value::Null)
            }
            "load" => {
                self.load(params)?;
                Ok(Value::Null)
            }
            "options" => {
                self.view_cache.clear();
                if let Some(opts) = params.as_object() {
                    for (k, v) in opts {
                        self.options[k] = v.clone();
                    }
                }
                self.dirty.extend(self.attached.keys());
                self.dirty.extend(self.cells.iter().map(|cell| cell.buf));
                Ok(Value::Null)
            }
            _ => bail!("Unknown engine operation: {method}"),
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for kernel in self.kernels.values_mut() {
            let _ = kernel.shutdown();
        }
    }
}
