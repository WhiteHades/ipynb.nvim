use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::time::Instant;

use super::{Engine, point};

#[derive(Clone)]
pub(super) struct Cell {
    pub(super) id: u64,
    pub(super) buf: u64,
    pub(super) kernel: String,
    pub(super) begin: [usize; 2],
    pub(super) end: [usize; 2],
    pub(super) source: String,
    pub(super) notebook_id: Option<String>,
    pub(super) outputs: Vec<Value>,
    pub(super) count: Value,
    pub(super) status: &'static str,
    pub(super) success: bool,
    pub(super) old: bool,
    pub(super) clear_next: bool,
    pub(super) started: Option<Instant>,
    pub(super) elapsed: f64,
}

impl Engine {
    pub(super) fn sync_positions(&mut self, params: &Value) {
        if let Some(positions) = params["positions"].as_array() {
            for pos in positions {
                let Some(id) = pos["id"].as_u64() else {
                    continue;
                };
                if let Some(cell) = self.cells.iter_mut().find(|c| c.id == id) {
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

    pub(super) fn new_cell(
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
        let notebook_id = self
            .cells
            .iter()
            .find(|c| Some(c.id) == old_id)
            .and_then(|c| c.notebook_id.clone());
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
            notebook_id,
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

    pub(super) fn prune_requests(&mut self) {
        let live: std::collections::HashSet<u64> = self.cells.iter().map(|c| c.id).collect();
        self.requests.retain(|_, id| live.contains(id));
        self.view_cache.retain(|id, _| live.contains(id));
    }

    pub(super) fn execute_cell(&mut self, id: u64) -> Result<()> {
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
}
