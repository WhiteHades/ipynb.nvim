use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::kernel::Kernel;
use crate::notebook::{self, Converter};

#[derive(Debug)]
pub struct SourceComparison(pub BTreeMap<String, Vec<String>>);
impl std::fmt::Display for SourceComparison {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Source comparison is required")
    }
}
impl std::error::Error for SourceComparison {}

#[derive(Clone)]
struct Cell {
    id: u64,
    buf: u64,
    kernel: String,
    begin: [usize; 2],
    end: [usize; 2],
    source: String,
    outputs: Vec<Value>,
    count: Value,
    status: &'static str,
    success: bool,
    old: bool,
    clear_next: bool,
    started: Option<Instant>,
    elapsed: f64,
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
    read_cache: Option<(PathBuf, std::time::SystemTime, Value)>,
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

fn comparison_sources(
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

fn imported_status(count: &Value) -> &'static str {
    if count.as_i64().is_some_and(|value| value != 0)
        || count.as_u64().is_some_and(|value| value != 0)
    {
        "done"
    } else {
        "new"
    }
}

fn same_source(left: &str, right: &str, language: &str, normalized: &Value) -> Option<bool> {
    if left == right {
        return Some(true);
    }
    let sources = normalized.get(language)?;
    Some(sources.get(left)? == sources.get(right)?)
}

fn positioned_sources(
    converter: &mut Converter,
    buffer: &str,
    starts: &[(u64, [usize; 2])],
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
    // Let Jupytext locate the spans; source searches also match prose and duplicate cells.
    let converted = converter.convert(&marked, None)?;
    let mut sources = Vec::new();
    let mut positions = HashMap::new();
    for cell in converted["cells"]
        .as_array()
        .context("Notebook cells must be an array")?
    {
        if cell["cell_type"] != "code" {
            continue;
        }
        let mut source = text(&cell["source"]);
        for (_, id, marker) in &insertions {
            if source.contains(marker) {
                if positions.insert(*id, sources.len()).is_some() {
                    bail!("Ambiguous notebook output position; file was not changed");
                }
                source = source.replace(marker, "");
            }
        }
        sources.push(source);
    }
    Ok((sources, positions))
}

impl Engine {
    pub fn new(python: String, converter: &Path) -> Result<Self> {
        Ok(Self {
            converter: Converter::new(&python, converter)?,
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
        })
    }

    fn selected_kernel(&self, params: &Value) -> Result<String> {
        let buf = params["buf"].as_u64().unwrap_or(0);
        let ids = self
            .attached
            .get(&buf)
            .ok_or_else(|| anyhow!("Initialize a kernel with :IpynbInit"))?;
        if let Some(id) = params["kernel"].as_str().filter(|s| !s.is_empty()) {
            if ids.iter().any(|item| item == id) {
                return Ok(id.to_owned());
            }
            bail!("Kernel {id} is not attached to this notebook");
        }
        if ids.len() != 1 {
            bail!("Select a kernel when multiple kernels are attached");
        }
        Ok(ids[0].clone())
    }

    fn kernel_specifications(&mut self) -> Result<serde_json::Map<String, Value>> {
        self.converter
            .call(&json!({"op":"kernelspecs"}))?
            .as_object()
            .cloned()
            .context("Kernel specifications must be an object")
    }

    fn sync_positions(&mut self, params: &Value) {
        if let Some(positions) = params["positions"].as_array() {
            for pos in positions {
                if let Some(cell) = self
                    .cells
                    .iter_mut()
                    .find(|c| Some(c.id) == pos["id"].as_u64())
                {
                    if cell.begin != point(&pos["begin"])
                        || cell.end != point(&pos["end"])
                        || pos["source"]
                            .as_str()
                            .is_some_and(|source| cell.source != source)
                    {
                        self.view_cache.remove(&cell.id);
                    }
                    cell.begin = point(&pos["begin"]);
                    cell.end = point(&pos["end"]);
                    if let Some(source) = pos["source"].as_str() {
                        cell.source = source.into();
                    }
                }
            }
        }
    }

    fn state(&self) -> Value {
        let running: BTreeMap<u64, Vec<&String>> = self
            .attached
            .iter()
            .map(|(buf, ids)| {
                (
                    *buf,
                    ids.iter()
                        .filter(|id| self.kernels.contains_key(*id))
                        .collect(),
                )
            })
            .collect();
        json!({"kernels": self.kernels.keys().collect::<Vec<_>>(), "buffers": running,
            "attachments": self.attached, "details": self.kernel_details})
    }

    fn new_cell(
        &mut self,
        buf: u64,
        kernel: String,
        begin: [usize; 2],
        end: [usize; 2],
        source: String,
    ) -> Result<u64> {
        if self
            .cells
            .iter()
            .any(|c| c.buf == buf && c.begin < end && begin < c.end && c.status == "running")
        {
            bail!("Cannot replace a running cell; interrupt it first");
        }
        let old_id = self
            .cells
            .iter()
            .find(|c| c.buf == buf && c.begin == begin && c.end == end)
            .map(|c| c.id);
        self.cells.retain(|c| {
            c.buf != buf || !(c.begin < end && begin < c.end) && !(c.begin == begin && c.end == end)
        });
        self.prune_requests();
        let id = old_id.unwrap_or_else(|| {
            let id = self.next_cell;
            self.next_cell += 1;
            id
        });
        self.view_cache.remove(&id);
        self.cells.push(Cell {
            id,
            buf,
            kernel,
            begin,
            end,
            source,
            outputs: vec![],
            count: Value::Null,
            status: "new",
            success: true,
            old: false,
            clear_next: false,
            started: None,
            elapsed: 0.0,
        });
        self.dirty.insert(buf);
        Ok(id)
    }

    fn prune_requests(&mut self) {
        let live: std::collections::HashSet<u64> = self.cells.iter().map(|c| c.id).collect();
        self.requests.retain(|_, id| live.contains(id));
        self.view_cache.retain(|id, _| live.contains(id));
    }

    fn execute_cell(&mut self, id: u64) -> Result<()> {
        let cell = self
            .cells
            .iter_mut()
            .find(|c| c.id == id)
            .context("Cell no longer exists")?;
        if cell.status == "running" {
            bail!("Cell is already running");
        }
        let kernel = self
            .kernels
            .get_mut(&cell.kernel)
            .context("Kernel is unavailable")?;
        let request = kernel.execute(&cell.source)?;
        self.view_cache.remove(&id);
        cell.outputs.clear();
        cell.count = Value::Null;
        cell.status = "hold";
        cell.success = true;
        cell.old = false;
        cell.clear_next = false;
        cell.elapsed = 0.0;
        cell.started = None;
        self.requests.retain(|_, cell_id| *cell_id != id);
        self.requests.insert(request, id);
        self.dirty.insert(cell.buf);
        Ok(())
    }

    pub fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value> {
        self.sync_positions(params);
        let buf = params["buf"].as_u64().unwrap_or(0);
        match method {
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
            "init" => {
                let name = params["kernel"]
                    .as_str()
                    .context("Kernel name is required")?;
                let id = if params["shared"].as_bool().unwrap_or(false) {
                    if !self.kernels.contains_key(name) {
                        bail!("No running kernel named {name}");
                    }
                    name.to_owned()
                } else {
                    let mut id = name.to_owned();
                    let mut index = 1;
                    while self.kernel_names.contains_key(&id) {
                        id = format!("{name}({index})");
                        index += 1;
                    }
                    let numpy = self.options["numpy_legacy_repr"].as_bool().unwrap_or(true);
                    let cwd = params["cwd"].as_str().map(Path::new);
                    if let Some(cwd) = cwd
                        && (!cwd.is_absolute() || !cwd.is_dir())
                    {
                        bail!("Kernel working directory must be an existing absolute directory");
                    }
                    let project_python = params["python"].as_str();
                    let path = Path::new(name);
                    let spec_path = if let Some(python) = project_python {
                        if name != "project"
                            || !Path::new(python).is_absolute()
                            || !Path::new(python).is_file()
                        {
                            bail!("Project kernel requires an absolute Python executable");
                        }
                        let check = std::process::Command::new(python)
                            .args(["-c", "import ipykernel_launcher"])
                            .output()
                            .context(
                                "Check project Python; run uv sync before executing notebook cells",
                            )?;
                        if !check.status.success() {
                            bail!(
                                "Project Python has no working ipykernel: {python}. Run uv sync in the project, or install ipykernel in that environment"
                            );
                        }
                        let spec = self
                            .assets
                            .path()
                            .join(format!("project-{}.json", uuid::Uuid::new_v4()));
                        std::fs::write(
                            &spec,
                            serde_json::to_vec(&json!({
                                "argv": [python, "-m", "ipykernel_launcher", "-f", "{connection_file}"],
                                "language": "python", "display_name": "Project Python"
                            }))?,
                        )?;
                        Some(spec)
                    } else if name.starts_with("http://")
                        || name.starts_with("https://")
                        || path.exists()
                        || name.ends_with(".json")
                    {
                        None
                    } else {
                        let specifications = self.kernel_specifications()?;
                        let resource = specifications
                            .get(name)
                            .and_then(Value::as_str)
                            .with_context(|| format!("No kernelspec named {name}"))?;
                        Some(PathBuf::from(resource).join("kernel.json"))
                    };
                    let interpreter = spec_path
                        .as_ref()
                        .and_then(|path| notebook::read_json(path).ok())
                        .and_then(|spec| spec["argv"][0].as_str().map(str::to_string))
                        .map(|python| {
                            if matches!(python.as_str(), "python" | "python3") {
                                self.python.clone()
                            } else {
                                python
                            }
                        });
                    let kernel =
                        Kernel::start(name, Some(&self.python), numpy, spec_path.as_deref(), cwd)?;
                    if let Some(language) = kernel.language() {
                        self.languages.insert(id.clone(), language.to_owned());
                    }
                    self.kernel_names.insert(id.clone(), name.to_owned());
                    self.kernel_details.insert(id.clone(), json!({
                        "kernel": name, "python": interpreter, "cwd": spec_path.as_ref().and(cwd),
                        "reason": params["reason"], "language": kernel.language()
                    }));
                    self.kernels.insert(id.clone(), kernel);
                    id
                };
                if let Some(previous) = params["replace"]
                    .as_str()
                    .filter(|old| !self.kernels.contains_key(*old))
                {
                    for (attached_buf, ids) in &mut self.attached {
                        if !ids.iter().any(|kernel| kernel == previous) {
                            continue;
                        }
                        for kernel in ids.iter_mut().filter(|kernel| kernel.as_str() == previous) {
                            *kernel = id.clone();
                        }
                        let mut seen = BTreeSet::new();
                        ids.retain(|kernel| seen.insert(kernel.clone()));
                        for cell in self
                            .cells
                            .iter_mut()
                            .filter(|cell| cell.buf == *attached_buf && cell.kernel == previous)
                        {
                            cell.kernel = id.clone();
                            self.view_cache.remove(&cell.id);
                        }
                        self.dirty.insert(*attached_buf);
                    }
                }
                let attached = self.attached.entry(buf).or_default();
                attached.retain(|id| self.kernels.contains_key(id));
                if !attached.contains(&id) {
                    attached.push(id.clone());
                }
                for cell in self.cells.iter_mut().filter(|cell| cell.buf == buf) {
                    if !self.kernels.contains_key(&cell.kernel) {
                        cell.kernel = id.clone();
                        self.view_cache.remove(&cell.id);
                        self.dirty.insert(buf);
                    }
                }
                self.events
                    .push(json!({"event":"state", "state":self.state()}));
                Ok(json!(id))
            }
            "deinit" | "unload" => {
                let detached = self.attached.remove(&buf).unwrap_or_default();
                self.cells.retain(|c| c.buf != buf);
                self.prune_requests();
                for id in detached {
                    if !self.attached.values().any(|ids| ids.contains(&id))
                        && let Some(kernel) = self.kernels.remove(&id)
                    {
                        kernel.shutdown()?;
                    }
                }
                self.dirty.insert(buf);
                self.events
                    .push(json!({"event":"state", "state":self.state()}));
                Ok(Value::Null)
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
            "read" => {
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
            "write" => self.write(params),
            "import" | "preview" => {
                self.import(params)?;
                Ok(Value::Null)
            }
            "export" => {
                self.export(params)?;
                Ok(Value::Null)
            }
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

    fn path(params: &Value) -> Result<&Path> {
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

    fn merge_outputs(
        &mut self,
        notebook: &mut Value,
        buf: u64,
        kernel: Option<&str>,
        normalized: &Value,
        buffer: Option<&str>,
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

    fn write(&mut self, params: &Value) -> Result<Value> {
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

    fn preserve_cell_identity(
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

    fn export(&mut self, params: &Value) -> Result<()> {
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

    fn import(&mut self, params: &Value) -> Result<()> {
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

    pub fn poll(&mut self) -> Result<()> {
        let messages: Vec<(String, Value)> = self
            .kernels
            .iter_mut()
            .flat_map(|(id, k)| k.drain().into_iter().map(|v| (id.clone(), v)))
            .collect();
        for (kernel, message) in messages {
            self.handle_message(&kernel, &message);
        }
        Ok(())
    }

    fn handle_message(&mut self, kernel: &str, message: &Value) {
        let kind = message["header"]["msg_type"]
            .as_str()
            .or(message["msg_type"].as_str())
            .unwrap_or("");
        let content = &message["content"];
        if kind == "ready" {
            self.events.push(json!({"event":"ready", "kernel":kernel}));
            return;
        }
        if kind == "kernel_info_reply" {
            if let Some(language) = content["language_info"]["name"].as_str() {
                self.languages
                    .insert(kernel.to_owned(), language.to_owned());
            }
            return;
        }
        if kind == "input_request" {
            self.events.push(json!({"event":"input", "kernel":kernel, "prompt":content["prompt"], "password":content["password"]}));
            return;
        }
        if kind == "transport_error" || kind == "kernel_stopped" {
            self.events
                .push(json!({"event":"error", "message":content["error"]}));
            for cell in self
                .cells
                .iter_mut()
                .filter(|c| c.kernel == kernel && matches!(c.status, "hold" | "running"))
            {
                cell.status = "done";
                cell.success = false;
                cell.elapsed = cell
                    .started
                    .take()
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(cell.elapsed);
                cell.clear_next = false;
                self.view_cache.remove(&cell.id);
                self.dirty.insert(cell.buf);
            }
            if kind == "kernel_stopped" {
                self.kernels.remove(kernel);
                let ids: std::collections::HashSet<u64> = self
                    .cells
                    .iter()
                    .filter(|cell| cell.kernel == kernel)
                    .map(|cell| cell.id)
                    .collect();
                self.requests.retain(|_, id| !ids.contains(id));
                for cell in self.cells.iter_mut().filter(|cell| cell.kernel == kernel) {
                    cell.old = true;
                    self.view_cache.remove(&cell.id);
                    self.dirty.insert(cell.buf);
                }
                // Keep associations and save metadata so stopped results remain usable.
                self.events
                    .push(json!({"event":"state", "state":self.state()}));
            }
            return;
        }
        if kind == "update_display_data" {
            if let Some(display_id) = content["transient"]["display_id"].as_str() {
                for cell in self.cells.iter_mut().filter(|c| c.kernel == kernel) {
                    for output in &mut cell.outputs {
                        if output["transient"]["display_id"].as_str() == Some(display_id) {
                            output["data"] = content["data"].clone();
                            output["metadata"] = content["metadata"].clone();
                            self.view_cache.remove(&cell.id);
                            self.dirty.insert(cell.buf);
                        }
                    }
                }
            }
            return;
        }
        let parent = message["parent_header"]["msg_id"].as_str().unwrap_or("");
        let Some(id) = self.requests.get(parent).copied() else {
            return;
        };
        let Some(cell) = self.cells.iter_mut().find(|c| c.id == id) else {
            return;
        };
        match kind {
            "status" if content["execution_state"] == "busy" => {
                cell.status = "running";
                cell.started = Some(Instant::now());
            }
            "status" if content["execution_state"] == "idle" => {
                cell.status = "done";
                cell.elapsed = cell
                    .started
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
            }
            "execute_input" => cell.count = content["execution_count"].clone(),
            "execute_reply" => {
                if !content["execution_count"].is_null() {
                    cell.count = content["execution_count"].clone();
                }
                if content["status"] == "error" {
                    cell.success = false;
                }
                if content["status"] == "aborted" {
                    cell.status = "done";
                    cell.success = false;
                }
            }
            "clear_output" => {
                if content["wait"].as_bool().unwrap_or(false) {
                    cell.clear_next = true;
                } else {
                    cell.outputs.clear();
                    cell.clear_next = false;
                }
            }
            "stream" | "display_data" | "execute_result" | "error" => {
                if cell.clear_next {
                    cell.outputs.clear();
                    cell.clear_next = false;
                }
                let mut output = content.clone();
                output["output_type"] = json!(kind);
                if kind == "error" {
                    cell.success = false;
                }
                if kind == "execute_result" {
                    cell.count = content["execution_count"].clone();
                }
                let last = cell.outputs.last_mut();
                if kind == "stream"
                    && last.as_ref().is_some_and(|o| {
                        o["output_type"] == "stream" && o["name"] == output["name"]
                    })
                {
                    let previous = last.unwrap();
                    append_text(&mut previous["text"], &output["text"]);
                } else {
                    cell.outputs.push(output);
                }
            }
            _ => return,
        }
        self.view_cache.remove(&cell.id);
        self.dirty.insert(cell.buf);
    }

    pub fn events(&mut self) -> Result<Vec<Value>> {
        let dirty: Vec<u64> = self.dirty.iter().copied().collect();
        for buf in dirty {
            let ids: Vec<(usize, u64)> = self
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.buf == buf)
                .map(|(index, c)| (index, c.id))
                .collect();
            let mut views = Vec::with_capacity(ids.len());
            for (index, id) in ids {
                if !self.view_cache.contains_key(&id) {
                    let cell = self.cells[index].clone();
                    let view = self.view(&cell)?;
                    self.view_cache.insert(id, view);
                }
                views.push(self.view_cache[&id].clone());
            }
            self.events
                .push(json!({"event":"cells", "buf":buf, "cells":views}));
            self.dirty.remove(&buf);
        }
        Ok(std::mem::take(&mut self.events))
    }

    fn asset(&self, extension: &str, bytes: &[u8]) -> Result<PathBuf> {
        use sha2::{Digest, Sha256};
        let path = self
            .assets
            .path()
            .join(format!("{:x}.{extension}", Sha256::digest(bytes)));
        if !path.exists() {
            std::fs::write(&path, bytes)?;
        }
        Ok(path)
    }

    fn view(&mut self, cell: &Cell) -> Result<Value> {
        let mut segments = vec![];
        let mut lines = vec![];
        let mut images = vec![];
        let mut html = Value::Null;
        let has_images = self.options["image_provider"].as_str().unwrap_or("none") != "none";
        for output in &cell.outputs {
            let kind = output["output_type"].as_str().unwrap_or("");
            let data = &output["data"];
            if let Some(value) = data.get("text/html") {
                html = json!(self.asset("html", text(value).as_bytes())?);
            }
            if self.options["show_mimetype_debug"]
                .as_bool()
                .unwrap_or(false)
                && data.is_object()
            {
                let debug = format!(
                    "[DEBUG] Received mimetypes: {:?}",
                    data.as_object().unwrap().keys().collect::<Vec<_>>()
                );
                lines.push(debug.clone());
                segments.push(json!({"kind":"text","lines":[debug]}));
            }
            let mut image = None;
            if has_images {
                for mime in [
                    "image/svg+xml",
                    "application/vnd.plotly.v1+json",
                    "text/latex",
                ] {
                    if let Some(value) = data.get(mime) {
                        use sha2::{Digest, Sha256};
                        let key = serde_json::to_vec(&(mime, value))?;
                        let path = self
                            .assets
                            .path()
                            .join(format!("{:x}.png", Sha256::digest(&key)));
                        if path.is_file()
                            || self
                                .converter
                                .call(
                                    &json!({"op":"render", "mime":mime, "data":value, "path":path}),
                                )
                                .is_ok()
                        {
                            image =
                                Some(json!({"kind":"image", "path":path, "mimetype":"image/png"}));
                            break;
                        }
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            if has_images && image.is_none() {
                for mime in [
                    "image/svg+xml",
                    "image/png",
                    "image/jpeg",
                    "image/jpg",
                    "image/gif",
                    "image/webp",
                    "image/bmp",
                    "image/tiff",
                ] {
                    if let Some(value) = data.get(mime) {
                        let raw = text(value);
                        let bytes = if mime == "image/svg+xml" {
                            Ok(raw.into_bytes())
                        } else {
                            base64::engine::general_purpose::STANDARD.decode(
                                raw.bytes()
                                    .filter(|c| !c.is_ascii_whitespace())
                                    .collect::<Vec<_>>(),
                            )
                        };
                        if let Ok(bytes) = bytes {
                            let ext = if mime == "image/svg+xml" {
                                "svg"
                            } else {
                                mime.split('/').nth(1).unwrap()
                            };
                            image = Some(
                                json!({"kind":"image","path":self.asset(ext,&bytes)?,"mimetype":mime}),
                            );
                            break;
                        }
                    }
                }
            }
            if let Some(image) = image {
                images.push(image.clone());
                segments.push(image);
                continue;
            }
            let body = match kind {
                "stream" => text(&output["text"]),
                "error" => output["traceback"]
                    .as_array()
                    .map(|a| a.iter().map(text).collect::<Vec<_>>().join("\n"))
                    .unwrap_or_else(|| {
                        format!("{}: {}", text(&output["ename"]), text(&output["evalue"]))
                    }),
                _ => data.get("text/plain").map(text).unwrap_or_else(|| {
                    format!(
                        "<No usable MIMEtype! Received mimetypes {:?}>",
                        data.as_object()
                            .map(|o| o.keys().collect::<Vec<_>>())
                            .unwrap_or_default()
                    )
                }),
            };
            let rendered = render_control_chars(&strip_ansi(&body));
            let body_lines: Vec<_> = rendered
                .trim_end_matches('\n')
                .split('\n')
                .map(str::to_owned)
                .collect();
            lines.extend(body_lines.clone());
            segments.push(json!({"kind":"text","lines":body_lines}));
        }
        Ok(
            json!({"id":cell.id,"buf":cell.buf,"kernel":cell.kernel,"begin":cell.begin,"end":cell.end,
            "source":cell.source,"execution_count":cell.count,"status":cell.status,"success":cell.success,
            "old":cell.old,"elapsed":cell.elapsed,"lines":lines,"images":images,"html":html,"segments":segments}),
        )
    }

    fn save(&mut self, params: &Value) -> Result<()> {
        let kernel = self.selected_kernel(params)?;
        let buf = params["buf"].as_u64().context("Buffer required")?;
        let lines = params["lines"]
            .as_array()
            .context("Buffer lines required")?;
        let checksum = format!(
            "{:x}",
            md5::compute(lines.iter().map(text).collect::<Vec<_>>().join("\n"))
        );
        let cells: Vec<Value> = self.cells.iter().filter(|c| c.buf == buf && c.kernel == kernel).map(|cell| {
            let chunks: Vec<Value> = cell.outputs.iter().map(|output| {
                let mut extras = output.as_object().cloned().unwrap_or_default();
                let kind = extras.remove("output_type").unwrap_or(json!("display_data"));
                let mut data = extras.remove("data").unwrap_or(json!({}));
                let metadata = extras.remove("metadata").unwrap_or(json!({}));
                if kind == "stream" { data = json!({"text/plain":extras.remove("text").unwrap_or(json!(""))}); }
                json!({"data":data,"metadata":metadata,"output_type":kind,"extras":extras})
            }).collect();
            json!({"span":{"begin":{"lineno":cell.begin[0],"colno":cell.begin[1]},
                "end":{"lineno":cell.end[0],"colno":cell.end[1]}},
                "execution_count":cell.count,"status":match cell.status {"hold"=>0,"running"=>1,"done"=>2,_=>3},
                "success":cell.success,"chunks":chunks})
        }).collect();
        let snapshot = json!({"version":1,"kernel":self.kernel_names.get(&kernel).unwrap_or(&kernel),
            "content_checksum":checksum,"cells":cells});
        notebook::write_atomic(Self::path(params)?, &snapshot, None)?;
        Ok(())
    }

    fn load(&mut self, params: &Value) -> Result<()> {
        let buf = params["buf"].as_u64().context("Buffer required")?;
        if self.attached.contains_key(&buf) {
            bail!("Deinitialize this buffer before loading saved state");
        }
        let snapshot = notebook::read_json(Self::path(params)?)?;
        if snapshot["version"] != 1 {
            bail!("Unsupported saved state version");
        }
        let name = snapshot["kernel"]
            .as_str()
            .context("Saved kernel name is required")?;
        let lines: Vec<&str> = params["lines"]
            .as_array()
            .context("Buffer lines required")?
            .iter()
            .map(|v| v.as_str().context("Buffer line must be text"))
            .collect::<Result<_>>()?;
        let checksum = format!("{:x}", md5::compute(lines.join("\n")));
        if snapshot["content_checksum"] != checksum {
            bail!("Buffer contents' checksum does not match!");
        }
        let mut restored = vec![];
        for saved in snapshot["cells"]
            .as_array()
            .context("Saved cells must be an array")?
        {
            let position = |name: &str| -> Result<[usize; 2]> {
                let span = &saved["span"][name];
                let row = span["lineno"].as_u64().context("Invalid saved line")? as usize;
                let col = span["colno"].as_u64().context("Invalid saved column")? as usize;
                let line = lines.get(row).context("Saved cell is outside the buffer")?;
                if !line.is_char_boundary(col) {
                    bail!("Saved cell column is outside a UTF-8 boundary");
                }
                Ok([row, col])
            };
            let begin = position("begin")?;
            let end = position("end")?;
            if begin > end {
                bail!("Saved cell has reversed boundaries");
            }
            let mut source = lines[begin[0]..=end[0]]
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>();
            source.last_mut().unwrap().truncate(end[1]);
            source[0].drain(..begin[1]);
            let count = saved
                .get("execution_count")
                .context("Missing saved execution count")?
                .clone();
            if !count.is_null() && !count.is_i64() {
                bail!("Invalid saved execution count");
            }
            if !matches!(saved["status"].as_u64(), Some(0..=3)) {
                bail!("Invalid saved cell status");
            }
            let success = saved["success"]
                .as_bool()
                .context("Invalid saved success flag")?;
            let mut outputs = vec![];
            for chunk in saved["chunks"]
                .as_array()
                .context("Saved chunks must be an array")?
            {
                let data = chunk["data"]
                    .as_object()
                    .context("Saved chunk data must be an object")?;
                let metadata = chunk["metadata"]
                    .as_object()
                    .context("Saved chunk metadata must be an object")?;
                let mut output = match chunk.get("extras") {
                    None => serde_json::Map::new(),
                    Some(value) => value
                        .as_object()
                        .context("Saved chunk extras must be an object")?
                        .clone(),
                };
                let kind = chunk["output_type"].as_str().unwrap_or("display_data");
                output.insert("output_type".into(), json!(kind));
                match kind {
                    "stream" => {
                        if !metadata.is_empty() {
                            output.insert("metadata".into(), json!(metadata));
                        }
                        output.entry("name").or_insert(json!("stdout"));
                        output.insert(
                            "text".into(),
                            data.get("text/plain").cloned().unwrap_or(json!("")),
                        );
                    }
                    "error" => {
                        output.entry("ename").or_insert(json!("Error"));
                        output.entry("evalue").or_insert(json!(""));
                        let trace = output.entry("traceback").or_insert(json!([]));
                        if !trace.is_array() {
                            bail!("Saved traceback must be an array");
                        }
                    }
                    _ => {
                        output.insert("data".into(), json!(data));
                        output.insert("metadata".into(), json!(metadata));
                        if kind == "execute_result" {
                            output.entry("execution_count").or_insert(count.clone());
                        }
                    }
                }
                outputs.push(Value::Object(output));
            }
            restored.push((begin, end, source.join("\n"), count, success, outputs));
        }
        // Validate the complete file before starting a kernel or replacing UI state.
        let kernel = self
            .dispatch(
                "init",
                &json!({"buf":buf,"kernel":name,"shared":params["shared"]}),
            )?
            .as_str()
            .context("Kernel initialization failed")?
            .to_owned();
        for (begin, end, source, count, success, outputs) in restored {
            let id = self.next_cell;
            self.next_cell += 1;
            self.cells.push(Cell {
                id,
                buf,
                kernel: kernel.clone(),
                begin,
                end,
                source,
                count,
                success,
                outputs,
                status: "done",
                old: true,
                clear_next: false,
                started: None,
                elapsed: 0.0,
            });
        }
        self.dirty.insert(buf);
        Ok(())
    }
}

fn append_text(previous: &mut Value, incoming: &Value) {
    if !previous.is_string() {
        *previous = Value::String(text(previous));
    }
    let Value::String(previous) = previous else {
        unreachable!()
    };
    append_string(previous, incoming);
}

fn append_string(previous: &mut String, incoming: &Value) {
    match incoming {
        Value::String(value) => previous.push_str(value),
        Value::Array(values) => {
            for value in values {
                append_string(previous, value);
            }
        }
        _ => {}
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for kernel in self.kernels.values_mut() {
            let _ = kernel.shutdown();
        }
    }
}

fn strip_ansi(text: &str) -> String {
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    ANSI.get_or_init(|| regex::Regex::new(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])").unwrap())
        .replace_all(text, "")
        .into_owned()
}

fn render_control_chars(text: &str) -> String {
    let mut lines: Vec<Vec<char>> = vec![vec![]];
    let mut col: usize = 0;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\n' {
            let mut next = chars.clone();
            if next.peek() == Some(&'\x08') {
                while next.peek() == Some(&'\x08') {
                    next.next();
                }
                if next.peek() == Some(&'\r') {
                    continue;
                }
            }
        }
        match ch {
            '\n' => {
                lines.push(vec![]);
                col = 0;
            }
            '\r' => col = 0,
            '\x08' => col = col.saturating_sub(1),
            ch => {
                let line = lines.last_mut().unwrap();
                if col < line.len() {
                    line[col] = ch;
                } else {
                    line.push(ch);
                }
                col += 1;
            }
        }
    }
    lines
        .into_iter()
        .map(|line| line.into_iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(kernel: &str, source: &str) -> Cell {
        Cell {
            id: 1,
            buf: 1,
            kernel: kernel.into(),
            begin: [0, 0],
            end: [0, source.len()],
            source: source.into(),
            outputs: vec![],
            count: Value::Null,
            status: "done",
            success: true,
            old: false,
            clear_next: false,
            started: None,
            elapsed: 0.0,
        }
    }

    #[test]
    fn progress_and_unicode_render_like_terminal_text() {
        assert_eq!(render_control_chars("abc\rXY"), "XYc");
        assert_eq!(render_control_chars("abc\x08\x08XY"), "aXY");
        assert_eq!(render_control_chars("λβγ\r🙂"), "🙂βγ");
    }

    #[test]
    fn source_comparison_groups_every_source_by_kernel_language() {
        let python = cell("python3", "print(1) # current");
        let r = cell("ir", "x <- 1 # current");
        let cells = vec![&python, &r];
        let notebook = vec![
            json!({"cell_type":"code", "source":"print(1) # saved"}),
            json!({"cell_type":"markdown", "source":"ignored"}),
            json!({"cell_type":"code", "source":"x <- 1 # saved"}),
        ];
        let languages = HashMap::from([
            ("python3".into(), "python".into()),
            ("ir".into(), "r".into()),
        ]);

        let comparison = comparison_sources(&cells, &notebook, &languages).0;
        assert_eq!(
            comparison.keys().cloned().collect::<Vec<_>>(),
            ["python", "r"]
        );
        for sources in comparison.values() {
            assert_eq!(sources.len(), 4);
            assert!(sources.contains(&python.source));
            assert!(sources.contains(&r.source));
            assert!(sources.contains(&"print(1) # saved".into()));
            assert!(sources.contains(&"x <- 1 # saved".into()));
        }
    }

    #[test]
    fn imported_cells_without_an_execution_count_are_new() {
        assert_eq!(imported_status(&Value::Null), "new");
        assert_eq!(imported_status(&json!(0)), "new");
        assert_eq!(imported_status(&json!(1)), "done");
    }

    fn runtime_engine() -> Engine {
        // Text-only runtime checks do not call the converter.
        Engine::new("/bin/cat".into(), Path::new("/dev/null")).unwrap()
    }

    fn runtime_message(kind: &str, parent: &str, content: Value) -> Value {
        json!({"header":{"msg_type":kind}, "parent_header":{"msg_id":parent}, "content":content})
    }

    #[test]
    fn runtime_outputs_survive_errors_and_stream_views_stay_current() {
        {
            let directory = tempfile::tempdir().unwrap();
            let spec = directory.path().join("kernel.json");
            std::fs::write(
                &spec,
                json!({"argv":["/bin/true"],"language":"r"}).to_string(),
            )
            .unwrap();
            let mut engine = runtime_engine();
            engine.kernels.insert(
                "test".into(),
                Kernel::start("test", None, false, Some(&spec), None).unwrap(),
            );
            engine.attached.insert(1, vec!["test".into()]);
            engine.attached.insert(2, vec!["test".into()]);
            engine.languages.insert("test".into(), "r".into());
            engine
                .kernel_names
                .insert("test".into(), "original-spec".into());
            let details = json!({"python":"/project/bin/R", "cwd":"/project", "reason":"manual"});
            engine.kernel_details.insert("test".into(), details.clone());
            let mut running = cell("test", "print(1)");
            running.status = "running";
            running.started = Some(Instant::now());
            running.count = json!(7);
            running.clear_next = true;
            running.outputs =
                vec![json!({"output_type":"stream", "name":"stdout", "text":"unsaved\n"})];
            let mut completed = cell("test", "print(2)");
            completed.id = 2;
            completed.buf = 2;
            completed.outputs = vec![
                json!({"output_type":"display_data", "data":{"text/html":"<b>unsaved</b>", "text/plain":"unsaved"}, "metadata":{"custom":true}, "transient":{"display_id":"display"}}),
            ];
            let original_outputs = [running.outputs.clone(), completed.outputs.clone()];
            engine.cells = vec![running, completed];
            engine.requests.insert("running".into(), 1);
            engine.requests.insert("completed".into(), 2);
            let deadline = Instant::now() + std::time::Duration::from_secs(2);
            while engine.kernels.contains_key("test") && Instant::now() < deadline {
                engine.poll().unwrap();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(!engine.kernels.contains_key("test"));
            assert!(engine.requests.is_empty());
            assert_eq!(engine.attached[&1], ["test"]);
            assert_eq!(engine.attached[&2], ["test"]);
            assert_eq!(engine.languages["test"], "r");
            assert_eq!(engine.kernel_names["test"], "original-spec");
            assert_eq!(engine.state()["details"]["test"], details);
            for (cell, outputs) in engine.cells.iter().zip(original_outputs) {
                assert_eq!(cell.outputs, outputs);
                assert_eq!(cell.status, "done");
                assert!(cell.old);
            }
            assert!(!engine.cells[0].success);
            assert!(!engine.cells[0].clear_next);
            assert!(engine.cells[0].started.is_none());
            assert!(engine.cells[1].success);
            let events = engine.events().unwrap();
            assert!(events.iter().any(|event| event["event"] == "state"));
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event["event"] == "cells")
                    .count(),
                2
            );
            let snapshot = directory.path().join("saved.json");
            engine
                .save(&json!({"buf":1,"path":snapshot,"lines":["print(1)"]}))
                .unwrap();
            let saved = notebook::read_json(&snapshot).unwrap();
            assert_eq!(saved["kernel"], "original-spec");
            assert_eq!(saved["cells"][0]["execution_count"], 7);
            assert_eq!(
                saved["cells"][0]["chunks"][0]["data"]["text/plain"],
                "unsaved\n"
            );
            // Late replies cannot mutate preserved results after the terminal event.
            engine.handle_message(
                "test",
                &runtime_message("stream", "running", json!({"name":"stdout","text":"late"})),
            );
            assert_eq!(engine.cells[0].outputs[0]["text"], "unsaved\n");
            engine.attached.get_mut(&2).unwrap().push("test".into());
            engine.attached.insert(3, vec!["unrelated".into()]);
            let preserved: Vec<_> = engine
                .cells
                .iter()
                .map(|cell| (cell.outputs.clone(), cell.count.clone()))
                .collect();
            let replacement = engine
                .dispatch(
                    "init",
                    &json!({"buf":1,"kernel":"project",
            "python":"/bin/true","replace":"test"}),
                )
                .unwrap();
            let replacement = replacement.as_str().unwrap();
            assert_eq!(engine.state()["details"][replacement]["kernel"], "project");
            assert_eq!(engine.attached[&1], [replacement]);
            assert_eq!(engine.attached[&2], [replacement]);
            assert_eq!(engine.attached[&3], ["unrelated"]);
            for (cell, (outputs, count)) in engine.cells.iter().zip(preserved) {
                assert_eq!(cell.kernel, replacement);
                assert_eq!(cell.outputs, outputs);
                assert_eq!(cell.count, count);
            }
            let events = engine.events().unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event["event"] == "cells")
                    .count(),
                2
            );
            engine
                .dispatch(
                    "init",
                    &json!({"buf":1,"kernel":"project",
            "python":"/bin/true","replace":replacement}),
                )
                .unwrap();
            assert_eq!(engine.attached[&2], [replacement]);
            assert_eq!(engine.cells[1].kernel, replacement);
        }
        {
            let mut original = String::with_capacity(100_000);
            original.push_str("start\r");
            let allocation = original.as_ptr();
            let mut previous = Value::String(original);
            for _ in 0..10_000 {
                append_text(&mut previous, &json!("λ\n"));
            }
            assert_eq!(
                previous.as_str().unwrap(),
                format!("start\r{}", "λ\n".repeat(10_000))
            );
            let Value::String(value) = previous else {
                panic!("stream stopped being text")
            };
            assert_eq!(value.as_ptr(), allocation);
            let mut imported = json!(["first\n", "second"]);
            append_text(&mut imported, &json!(["\r", ["λ", "\n"]]));
            assert_eq!(imported, "first\nsecond\rλ\n");
        }
        {
            let mut engine = runtime_engine();
            let mut first = cell("test", "one");
            first.outputs = vec![json!({"output_type":"stream","name":"stdout","text":"one"})];
            let mut second = cell("test", "two");
            second.id = 2;
            second.begin = [1, 0];
            second.end = [1, 3];
            second.outputs = vec![json!({"output_type":"stream","name":"stdout","text":"two"})];
            engine.cells = vec![first, second];
            engine.requests.insert("one".into(), 1);
            engine.dirty.insert(1);
            engine.events().unwrap();
            // A sentinel proves the unchanged view was reused rather than rendered.
            engine.view_cache.get_mut(&2).unwrap()["reused"] = json!(true);
            engine.handle_message(
                "test",
                &runtime_message("stream", "one", json!({"name":"stdout","text":" added"})),
            );
            assert!(!engine.view_cache.contains_key(&1));
            assert!(engine.view_cache.contains_key(&2));
            let events = engine.events().unwrap();
            let views = events[0]["cells"].as_array().unwrap();
            assert_eq!(views.len(), 2);
            assert_eq!(views[0]["lines"], json!(["one added"]));
            assert_eq!(views[1]["reused"], true);
            engine.sync_positions(
                &json!({"positions":[{"id":2,"begin":[2,0],"end":[2,3],"source":"new"}]}),
            );
            assert!(!engine.view_cache.contains_key(&2));
            engine
                .dispatch("options", &json!({"show_mimetype_debug":true}))
                .unwrap();
            assert!(engine.view_cache.is_empty());
            assert!(engine.dirty.contains(&1));
            engine.events().unwrap();
            engine.view_cache.get_mut(&2).unwrap()["reused"] = json!(true);
            let directory = tempfile::tempdir().unwrap();
            let spec = directory.path().join("kernel.json");
            std::fs::write(
                &spec,
                json!({"argv":["/bin/sleep","60"],"language":"r"}).to_string(),
            )
            .unwrap();
            engine.kernels.insert(
                "rebound".into(),
                Kernel::start("rebound", None, false, Some(&spec), None).unwrap(),
            );
            engine
                .dispatch("init", &json!({"buf":1,"kernel":"rebound","shared":true}))
                .unwrap();
            let events = engine.events().unwrap();
            let views = &events
                .iter()
                .find(|event| event["event"] == "cells")
                .unwrap()["cells"];
            assert_eq!(views[1]["kernel"], "rebound");
            assert!(views[1]["reused"].is_null());
        }
        {
            let directory = tempfile::tempdir().unwrap();
            let spec = directory.path().join("kernel.json");
            std::fs::write(
                &spec,
                json!({"argv":["/bin/sleep","60"],"language":"r"}).to_string(),
            )
            .unwrap();
            let mut engine = runtime_engine();
            engine.kernels.insert(
                "test".into(),
                Kernel::start("test", None, false, Some(&spec), None).unwrap(),
            );
            engine.attached.insert(1, vec!["test".into()]);
            let mut pending = cell("test", "one");
            pending.status = "hold";
            engine.cells.push(pending);
            engine.requests.insert("one".into(), 1);
            engine.handle_message(
                "test",
                &runtime_message("transport_error", "", json!({"error":"bad frame"})),
            );
            assert!(engine.kernels.contains_key("test"));
            assert_eq!(engine.attached[&1], ["test"]);
            assert_eq!(engine.requests["one"], 1);
            assert!(!engine.cells[0].old);
            assert!(!engine.cells[0].success);
        }
        {
            let mut engine = runtime_engine();
            engine.cells.push(cell("test", "one"));
            engine.requests.insert("one".into(), 1);
            for (name, text) in [
                ("stdout", "one"),
                ("stdout", " two"),
                ("stderr", "error"),
                ("stdout", "three"),
            ] {
                engine.handle_message(
                    "test",
                    &runtime_message("stream", "one", json!({"name":name,"text":text})),
                );
            }
            assert_eq!(engine.cells[0].outputs.len(), 3);
            assert_eq!(engine.cells[0].outputs[0]["text"], "one two");
            assert_eq!(engine.cells[0].outputs[1]["text"], "error");
            assert_eq!(engine.cells[0].outputs[2]["text"], "three");
            engine.handle_message(
                "test",
                &runtime_message("clear_output", "one", json!({"wait":true})),
            );
            assert_eq!(engine.cells[0].outputs.len(), 3);
            engine.handle_message(
                "test",
                &runtime_message(
                    "stream",
                    "one",
                    json!({"name":"stdout","text":"replacement"}),
                ),
            );
            assert_eq!(engine.cells[0].outputs.len(), 1);
            assert_eq!(engine.cells[0].outputs[0]["text"], "replacement");
        }
    }

    fn save_engine() -> Result<Engine> {
        Engine::new(
            std::env::var("IPYNB_TEST_PYTHON").unwrap_or_else(|_| "python3".into()),
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("converter.py"),
        )
    }

    fn save_directory() -> Result<tempfile::TempDir> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".tmp");
        std::fs::create_dir_all(&root)?;
        Ok(tempfile::Builder::new()
            .prefix("save-check-")
            .tempdir_in(root)?)
    }

    fn save_duplicate_fixture(engine: &mut Engine, path: &Path) -> Result<(String, Value)> {
        let original = json!({"nbformat":4,"nbformat_minor":5,"metadata":{"custom":"keep"},
        "cells":[
            {"id":"prose","cell_type":"markdown","metadata":{"custom":true},
             "source":"print(1)\n\n![image](attachment:tiny.png)",
             "attachments":{"tiny.png":{"image/png":"aGVsbG8="}}},
            {"id":"first","cell_type":"code","metadata":{"custom":"first"},
             "source":"print(1)","execution_count":1,
             "outputs":[{"output_type":"stream","name":"stdout","text":"first\n"}]},
            {"id":"second","cell_type":"code","metadata":{"custom":"second"},
             "source":"print(1)","execution_count":2,
             "outputs":[{"output_type":"stream","name":"stdout","text":"second\n"}]}
        ]});
        let mtime = notebook::write_atomic(path, &original, None)?;
        let buffer = text(&engine.converter.read(path, Path::new(""))?["text"]);
        let row = buffer[..buffer.rfind("print(1)").unwrap()]
            .bytes()
            .filter(|b| *b == b'\n')
            .count();
        let id = engine.new_cell(1, "python3".into(), [row, 0], [row, 8], "print(1)".into())?;
        let cell = engine.cells.iter_mut().find(|cell| cell.id == id).unwrap();
        cell.outputs = vec![json!({"output_type":"stream","name":"stdout","text":"new\n"})];
        cell.count = json!(3);
        engine.attached.insert(1, vec!["python3".into()]);
        Ok((buffer, mtime["mtime"].clone()))
    }

    #[test]
    fn save_outputs_preserve_identity_and_reject_ambiguous_or_stale_files() -> Result<()> {
        for export in [false, true] {
            let directory = save_directory()?;
            let path = directory.path().join("duplicate.ipynb");
            let mut engine = save_engine()?;
            let (buffer, mtime) = save_duplicate_fixture(&mut engine, &path)?;
            let original = notebook::read_json(&path)?;
            if export {
                engine.export(&json!({"path":path,"buf":1,"overwrite":true,
                "lines":buffer.split('\n').collect::<Vec<_>>()}))?;
            } else {
                engine.write(&json!({"path":path,"buf":1,"text":buffer,"mtime":mtime}))?;
            }
            let saved = notebook::read_json(&path)?;
            assert_eq!(text(&saved["cells"][1]["outputs"][0]["text"]), "first\n");
            assert_eq!(text(&saved["cells"][2]["outputs"][0]["text"]), "new\n");
            assert_eq!(saved["cells"][1]["execution_count"], 1);
            assert_eq!(saved["cells"][2]["execution_count"], 3);
            for index in 0..3 {
                assert_eq!(saved["cells"][index]["id"], original["cells"][index]["id"]);
                assert_eq!(
                    saved["cells"][index]["metadata"],
                    original["cells"][index]["metadata"]
                );
            }
            assert_eq!(
                saved["cells"][0]["attachments"],
                original["cells"][0]["attachments"]
            );
            assert_eq!(saved["metadata"]["custom"], "keep");
        }
        {
            let mut engine = save_engine()?;
            let buffer = "```python\n\n```\n\n```python\nsecond = 2\n```\n\n```python\nfirst = 1 # live\n```";
            let mut notebook = engine.converter.convert(buffer, None)?;
            let originals = notebook["cells"].as_array().unwrap();
            assert_eq!(originals.len(), 3);
            let mut moved = vec![
                originals[2].clone(),
                originals[0].clone(),
                originals[1].clone(),
            ];
            moved[0]["source"] = json!("first = 1 # disk");
            moved[0]["metadata"] = json!({"custom":"keep"});
            notebook["cells"] = json!(moved);
            for (row, source, count) in
                [(1, "", 1), (5, "second = 2", 2), (9, "first = 1 # live", 3)]
            {
                let id = engine.new_cell(
                    1,
                    "python3".into(),
                    [row, 0],
                    [row, source.len()],
                    source.into(),
                )?;
                let cell = engine.cells.iter_mut().find(|cell| cell.id == id).unwrap();
                cell.count = json!(count);
                cell.outputs = vec![
                    json!({"output_type":"execute_result","execution_count":null,
                "data":{"text/plain":format!("{count}\n")},"metadata":{},"transient":{"display_id":"local"}}),
                ];
            }
            let normalized = json!({"python":{"":"","second = 2":"second = 2\n",
            "first = 1 # live":"first = 1\n","first = 1 # disk":"first = 1\n"}});
            let disk = notebook.clone();
            let comparison = engine
                .merge_outputs(&mut notebook, 1, None, &json!({}), Some(buffer))
                .unwrap_err();
            assert!(comparison.downcast_ref::<SourceComparison>().is_some());
            assert_eq!(notebook, disk);
            assert!(engine.merge_outputs(&mut notebook, 1, None, &normalized, Some(buffer))?);
            for (index, count) in [(0, 3), (1, 1), (2, 2)] {
                assert_eq!(notebook["cells"][index]["execution_count"], count);
                assert_eq!(
                    notebook["cells"][index]["outputs"][0]["execution_count"],
                    count
                );
                assert!(
                    notebook["cells"][index]["outputs"][0]
                        .get("transient")
                        .is_none()
                );
            }
            assert_eq!(notebook["cells"][0]["metadata"]["custom"], "keep");
            let directory = save_directory()?;
            let path = directory.path().join("moved.ipynb");
            let mtime = notebook::write_atomic(&path, &disk, None)?;
            engine.write(
                &json!({"path":path,"buf":1,"text":buffer,"mtime":mtime,"normalized":normalized}),
            )?;
            let saved = notebook::read_json(&path)?;
            for (index, previous, count) in [(0, 1, 1), (1, 2, 2), (2, 0, 3)] {
                assert_eq!(saved["cells"][index]["id"], disk["cells"][previous]["id"]);
                assert_eq!(saved["cells"][index]["execution_count"], count);
            }
            assert_eq!(saved["cells"][2]["metadata"]["custom"], "keep");
        }
        {
            let mut engine = save_engine()?;
            engine.new_cell(1, "python3".into(), [4, 0], [4, 8], "print(1)".into())?;
            let mut notebook = json!({"cells":[
            {"cell_type":"code","source":"print(1)","outputs":[],"execution_count":1},
            {"cell_type":"code","source":"print(1)","outputs":[],"execution_count":2}]});
            let original = notebook.clone();
            let error = engine
                .merge_outputs(&mut notebook, 1, None, &json!({}), None)
                .unwrap_err();
            assert!(error.to_string().contains("Ambiguous"));
            assert_eq!(notebook, original);
            let buffer = "```python\nprint(1) # new\n```\n\n```python\nprint(1) # new\n```";
            engine.cells[0].begin = [5, 0];
            engine.cells[0].source = "print(1) # new".into();
            let normalized =
                json!({"python":{"print(1)":"print(1)\n","print(1) # new":"print(1)\n"}});
            let error = engine
                .merge_outputs(&mut notebook, 1, None, &normalized, Some(buffer))
                .unwrap_err();
            assert!(error.to_string().contains("Ambiguous"));
            assert_eq!(notebook, original);
            let originals =
                vec![json!({"id":"old","cell_type":"code","source":"print(1)","metadata":{}})];
            let mut converted = json!({"cells":[
            {"id":"new-a","cell_type":"code","source":"print(1) # new","metadata":{}},
            {"id":"new-b","cell_type":"code","source":"print(1) # new","metadata":{}}]});
            let unchanged = converted.clone();
            let error = engine
                .preserve_cell_identity(&mut converted, &originals, 1, &normalized)
                .unwrap_err();
            assert!(error.to_string().contains("Ambiguous"));
            assert_eq!(converted, unchanged);
            engine.cells[0].begin = [0, 0];
            engine.cells[0].source = "print(1)".into();
            let mut prose_only = json!({"cells":[
            {"cell_type":"code","source":"print(1)","outputs":[],"execution_count":1}]});
            let unchanged = prose_only.clone();
            let error = engine
                .merge_outputs(&mut prose_only, 1, None, &json!({}), Some("print(1)"))
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("No notebook cell at output line")
            );
            assert_eq!(prose_only, unchanged);
        }
        {
            let mut engine = save_engine()?;
            let buffer = "print(1)\n\n```python\nprint(1)\n```\n\n```python\n\n```\n\n```python\nprint(1)\n```";
            let mut saved = engine.converter.convert(buffer, None)?;
            let code_cells: Vec<_> = saved["cells"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .filter(|cell| cell["cell_type"] == "code")
                .collect();
            assert_eq!(code_cells.len(), 3);
            for (index, cell) in code_cells.into_iter().enumerate() {
                cell["execution_count"] = json!(index + 1);
                cell["outputs"] =
                    json!([{"output_type":"stream","name":"stdout","text":format!("{index}\n")}]);
            }
            let directory = save_directory()?;
            let path = directory.path().join("preview.ipynb");
            notebook::write_atomic(&path, &saved, None)?;
            let spans = json!([
            {"begin":[3,0],"end":[3,8]}, {"begin":[7,0],"end":[7,0]},
            {"begin":[11,0],"end":[11,8]}]);
            engine.import(&json!({"buf":1,"path":path,"lines":buffer.split('\n').collect::<Vec<_>>(),"spans":spans}))?;
            assert!(engine.kernels.is_empty());
            assert!(engine.attached.is_empty());
            assert_eq!(engine.cells.len(), 3);
            for (index, row) in [3, 7, 11].into_iter().enumerate() {
                assert_eq!(engine.cells[index].begin, [row, 0]);
                assert_eq!(engine.cells[index].count, index + 1);
                assert_eq!(
                    text(&engine.cells[index].outputs[0]["text"]),
                    format!("{index}\n")
                );
            }
            let mut edited = save_engine()?;
            let changed = buffer.replacen("```python\nprint(1)", "```python\nprint(2)", 1);
            edited.import(&json!({"buf":1,"path":path,"lines":changed.split('\n').collect::<Vec<_>>(),"spans":spans}))?;
            assert_eq!(edited.cells.len(), 2);
            assert_eq!(edited.cells[0].begin, [7, 0]);
            assert_eq!(edited.cells[1].begin, [11, 0]);
            assert_eq!(edited.cells[1].count, 3);
        }
        {
            let mut engine = save_engine()?;
            let buffer = "```python\nvalue = 1 # second\n```\n\n```python\nvalue = 1 # first\n```";
            let mut notebook = json!({"cells":[
            {"cell_type":"code","source":"value = 1 # first","outputs":[],"execution_count":1},
            {"cell_type":"code","source":"value = 1 # second","outputs":[],"execution_count":2}]});
            engine.new_cell(
                1,
                "python3".into(),
                [5, 0],
                [5, 17],
                "value = 1 # first".into(),
            )?;
            engine.cells[0].count = json!(9);
            let normalized = json!({"python":{"value = 1 # first":"value = 1\n",
            "value = 1 # second":"value = 1\n"}});
            assert!(engine.merge_outputs(&mut notebook, 1, None, &normalized, Some(buffer))?);
            assert_eq!(notebook["cells"][0]["execution_count"], 9);
            assert_eq!(notebook["cells"][1]["execution_count"], 2);
        }
        #[cfg(unix)]
        for during_read in [true, false] {
            let directory = save_directory()?;
            let path = directory.path().join("race.ipynb");
            let script = directory.path().join("converter.sh");
            let original = json!({"nbformat":if during_read {3} else {4},"cells":[
                {"cell_type":"code","source":"print(1)","outputs":[],"execution_count":null}]});
            std::fs::write(&path, serde_json::to_vec(&original)?)?;
            std::fs::File::options()
                .write(true)
                .open(&path)?
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
                )?;
            let response = if during_read {
                "printf '%s\\n' '{\"ok\":true,\"result\":{\"notebook\":{\"nbformat\":4,\"cells\":[]}}}'\n".to_owned()
            } else {
                "marker=$(printf '%s' \"$request\" | sed -n 's/.*\\(__ipynb_[a-f0-9]*_[0-9]*__\\).*/\\1/p')\nprintf '{\"ok\":true,\"result\":{\"cells\":[{\"cell_type\":\"code\",\"source\":\"%sprint(1)\"}]}}\\n' \"$marker\"\n".to_owned()
            };
            let quoted_path = path.to_string_lossy().replace('\'', "'\\''");
            std::fs::write(
                &script,
                format!(
                    "IFS= read -r request\nprintf '%s\\n' '{{\"external\":true}}' > '{quoted_path}'\n{response}"
                ),
            )?;
            let mut engine = Engine::new("sh".into(), &script)?;
            engine.attached.insert(1, vec!["python3".into()]);
            engine.new_cell(1, "python3".into(), [1, 0], [1, 8], "print(1)".into())?;
            engine.cells[0].outputs =
                vec![json!({"output_type":"stream","name":"stdout","text":"new\n"})];
            let error = engine
                .export(&json!({"path":path,"buf":1,"overwrite":true,
                "lines":["```python","print(1)","```"]}))
                .unwrap_err();
            assert!(error.to_string().contains("changed on disk"), "{error:#}");
            assert_eq!(std::fs::read_to_string(&path)?, "{\"external\":true}\n");
        }
        Ok(())
    }
}
