use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::Engine;
use crate::{kernel::Kernel, notebook};

impl Engine {
    pub(super) fn selected_kernel(&self, params: &Value) -> Result<String> {
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

    pub(super) fn kernel_specifications(&self) -> Result<serde_json::Map<String, Value>> {
        crate::kernelspec::discover(&self.python)
    }

    fn python_specification(&self, python: &str, display: &str) -> Result<PathBuf> {
        let path = self
            .assets
            .path()
            .join(format!("python-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "argv": [python, "-m", "ipykernel_launcher", "-f", "{connection_file}"],
                "language": "python", "display_name": display
            }))?,
        )?;
        Ok(path)
    }

    pub(super) fn state(&self) -> Value {
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

    pub(super) fn init(&mut self, params: &Value) -> Result<Value> {
        let buf = params["buf"].as_u64().unwrap_or(0);
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
                    .context("Check project Python; run uv sync before executing notebook cells")?;
                if !check.status.success() {
                    bail!(
                        "Project Python has no working ipykernel: {python}. Run uv sync in the project, or install ipykernel in that environment"
                    );
                }
                Some(self.python_specification(python, "Project Python")?)
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
                let resource = Path::new(resource);
                let spec = resource.join("kernel.json");
                if name == "python3" && !spec.is_file() && resource.ends_with("ipykernel/resources")
                {
                    Some(self.python_specification(&self.python, "Python (ipynb.nvim)")?)
                } else {
                    Some(spec)
                }
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
            let kernel = Kernel::start(name, Some(&self.python), numpy, spec_path.as_deref(), cwd)?;
            if let Some(language) = kernel.language() {
                self.languages.insert(id.clone(), language.to_owned());
            }
            self.kernel_names.insert(id.clone(), name.to_owned());
            self.kernel_details.insert(
                id.clone(),
                json!({
                    "kernel": name, "python": interpreter, "cwd": spec_path.as_ref().and(cwd),
                    "reason": params["reason"], "language": kernel.language()
                }),
            );
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

    pub(super) fn deinit(&mut self, buf: u64) -> Result<Value> {
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
}
