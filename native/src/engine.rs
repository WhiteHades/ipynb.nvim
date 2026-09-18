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
    next_cell: u64,
    options: Value,
    assets: tempfile::TempDir,
    read_cache: Option<(PathBuf, std::time::SystemTime, Value)>,
    dirty: BTreeSet<u64>,
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
            next_cell: 1,
            options: json!({}),
            assets: tempfile::Builder::new().prefix("ipynb-native-").tempdir()?,
            dirty: BTreeSet::new(),
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

    fn sync_positions(&mut self, params: &Value) {
        if let Some(positions) = params["positions"].as_array() {
            for pos in positions {
                if let Some(cell) = self
                    .cells
                    .iter_mut()
                    .find(|c| Some(c.id) == pos["id"].as_u64())
                {
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
        json!({"kernels": self.kernels.keys().collect::<Vec<_>>(), "buffers": self.attached})
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
                Ok(self.state())
            }
            "available" => {
                let specifications = self.converter.call(&json!({"op":"kernelspecs"}))?;
                let mut names: Vec<&String> = specifications
                    .as_object()
                    .context("Kernel specifications must be an object")?
                    .keys()
                    .collect();
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
                    while self.kernels.contains_key(&id) {
                        id = format!("{name}({index})");
                        index += 1;
                    }
                    let numpy = self.options["numpy_legacy_repr"].as_bool().unwrap_or(true);
                    let kernel = Kernel::start(name, Some(&self.python), numpy)?;
                    if let Some(language) = kernel.language() {
                        self.languages.insert(id.clone(), language.to_owned());
                    }
                    self.kernel_names.insert(id.clone(), name.to_owned());
                    self.kernels.insert(id.clone(), kernel);
                    id
                };
                let attached = self.attached.entry(buf).or_default();
                if !attached.contains(&id) {
                    attached.push(id.clone());
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
                    if !self.attached.values().any(|ids| ids.contains(&id)) {
                        if let Some(kernel) = self.kernels.remove(&id) {
                            kernel.shutdown()?;
                        }
                    }
                }
                self.dirty.insert(buf);
                self.events
                    .push(json!({"event":"state", "state":self.state()}));
                Ok(Value::Null)
            }
            "execute" | "define" => {
                let kernel = self.selected_kernel(params)?;
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
            "import" => {
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
                if let Some(opts) = params.as_object() {
                    for (k, v) in opts {
                        self.options[k] = v.clone();
                    }
                }
                self.dirty.extend(self.attached.keys());
                Ok(Value::Null)
            }
            _ => bail!("Unknown engine operation: {method}"),
        }
    }

    fn path<'a>(params: &'a Value) -> Result<&'a Path> {
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
        &self,
        notebook: &mut Value,
        buf: u64,
        kernel: Option<&str>,
        normalized: &Value,
    ) -> Result<bool> {
        let mut cells: Vec<_> = self
            .cells
            .iter()
            .filter(|c| c.buf == buf && kernel.is_none_or(|k| c.kernel == k))
            .collect();
        cells.sort_by_key(|c| c.begin);
        let nb_cells = notebook["cells"]
            .as_array_mut()
            .context("Notebook cells must be an array")?;
        let comparison = comparison_sources(&cells, nb_cells, &self.languages);
        let mut index = 0;
        let mut changed = false;
        for cell in cells {
            let mut matched = false;
            while index < nb_cells.len() {
                let nb = &mut nb_cells[index];
                index += 1;
                if nb["cell_type"] != "code" {
                    continue;
                }
                let source = text(&nb["source"]);
                if source != cell.source {
                    let language = self
                        .languages
                        .get(&cell.kernel)
                        .map(String::as_str)
                        .unwrap_or("python");
                    match (
                        normalized
                            .get(language)
                            .and_then(|items| items.get(&source)),
                        normalized
                            .get(language)
                            .and_then(|items| items.get(&cell.source)),
                    ) {
                        (Some(left), Some(right)) if left == right => {}
                        (Some(_), Some(_)) => continue,
                        _ => return Err(comparison.into()),
                    }
                }
                let mut outputs = cell.outputs.clone();
                for output in &mut outputs {
                    if let Some(obj) = output.as_object_mut() {
                        obj.remove("transient");
                    }
                    if output["output_type"] == "execute_result"
                        && output["execution_count"].is_null()
                    {
                        output["execution_count"] = cell.count.clone();
                    }
                    notebook::split_output(output);
                }
                changed |= nb["outputs"] != json!(outputs) || nb["execution_count"] != cell.count;
                nb["outputs"] = json!(outputs);
                nb["execution_count"] = cell.count.clone();
                matched = true;
                break;
            }
            if !matched {
                bail!(
                    "No notebook cell matches output at line {}; file was not changed",
                    cell.begin[0] + 1
                );
            }
        }
        Ok(changed)
    }

    fn write(&mut self, params: &Value) -> Result<Value> {
        let path = Self::path(params)?;
        let existing = if path.exists() {
            Some(notebook::read_json(path)?)
        } else {
            None
        };
        let mut notebook = self.converter.convert(&text(&params["text"]), existing)?;
        self.merge_outputs(
            &mut notebook,
            params["buf"].as_u64().unwrap_or(0),
            None,
            &params["normalized"],
        )?;
        notebook::split_lines(&mut notebook);
        notebook::write_atomic(
            path,
            &notebook,
            params.get("mtime").filter(|v| !v.is_null()),
        )
    }

    fn export(&mut self, params: &Value) -> Result<()> {
        let path = Self::path(params)?;
        let mut notebook = self.read_compatible_notebook(path)?;
        let kernel = self.selected_kernel(params)?;
        if !self
            .cells
            .iter()
            .any(|cell| cell.buf == params["buf"].as_u64().unwrap_or(0) && cell.kernel == kernel)
        {
            return Ok(());
        }
        let changed = self.merge_outputs(
            &mut notebook,
            params["buf"].as_u64().unwrap_or(0),
            Some(&kernel),
            &params["normalized"],
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
            notebook::write_atomic(&dest, &notebook, None)?;
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
        let kernel = self.selected_kernel(params)?;
        let lines: Vec<String> = params["lines"]
            .as_array()
            .context("Buffer lines are required")?
            .iter()
            .map(text)
            .collect();
        let mut offset = 0;
        for nb in notebook["cells"]
            .as_array()
            .context("Notebook cells missing")?
        {
            if nb["cell_type"] != "code" {
                continue;
            }
            let source = text(&nb["source"]);
            let code: Vec<&str> = source.split('\n').collect();
            let Some(start) = (offset..lines.len()).find(|&i| {
                i + code.len() <= lines.len()
                    && lines[i..i + code.len()]
                        .iter()
                        .zip(&code)
                        .all(|(a, b)| a == b)
            }) else {
                continue;
            };
            let end = start + code.len() - 1;
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
            let kind = message["header"]["msg_type"]
                .as_str()
                .or(message["msg_type"].as_str())
                .unwrap_or("");
            let content = &message["content"];
            if kind == "ready" {
                self.events.push(json!({"event":"ready", "kernel":kernel}));
                continue;
            }
            if kind == "kernel_info_reply" {
                if let Some(language) = content["language_info"]["name"].as_str() {
                    self.languages.insert(kernel.clone(), language.to_owned());
                }
                continue;
            }
            if kind == "input_request" {
                self.events.push(json!({"event":"input", "kernel":kernel, "prompt":content["prompt"], "password":content["password"]}));
                continue;
            }
            if kind == "transport_error" {
                self.events
                    .push(json!({"event":"error", "message":content["error"]}));
                for cell in self
                    .cells
                    .iter_mut()
                    .filter(|c| c.kernel == kernel && matches!(c.status, "hold" | "running"))
                {
                    cell.status = "done";
                    cell.success = false;
                    self.dirty.insert(cell.buf);
                }
                continue;
            }
            if kind == "update_display_data" {
                if let Some(display_id) = content["transient"]["display_id"].as_str() {
                    for cell in self.cells.iter_mut().filter(|c| c.kernel == kernel) {
                        for output in &mut cell.outputs {
                            if output["transient"]["display_id"].as_str() == Some(display_id) {
                                output["data"] = content["data"].clone();
                                output["metadata"] = content["metadata"].clone();
                                self.dirty.insert(cell.buf);
                            }
                        }
                    }
                }
                continue;
            }
            let parent = message["parent_header"]["msg_id"].as_str().unwrap_or("");
            let Some(id) = self.requests.get(parent).copied() else {
                continue;
            };
            let Some(cell) = self.cells.iter_mut().find(|c| c.id == id) else {
                continue;
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
                        previous["text"] = json!(text(&previous["text"]) + &text(&output["text"]));
                    } else {
                        cell.outputs.push(output);
                    }
                }
                _ => continue,
            }
            self.dirty.insert(cell.buf);
        }
        Ok(())
    }

    pub fn events(&mut self) -> Result<Vec<Value>> {
        let dirty = std::mem::take(&mut self.dirty);
        for buf in dirty {
            let cells: Vec<Cell> = self
                .cells
                .iter()
                .filter(|c| c.buf == buf)
                .cloned()
                .collect();
            let views: Result<Vec<Value>> = cells.iter().map(|c| self.view(c)).collect();
            self.events
                .push(json!({"event":"cells", "buf":buf, "cells":views?}));
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
        assert_eq!(comparison.keys().cloned().collect::<Vec<_>>(), ["python", "r"]);
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
}
