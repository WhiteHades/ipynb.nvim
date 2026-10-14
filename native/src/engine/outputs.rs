use anyhow::Result;
use serde_json::{Value, json};
use std::time::Instant;

use super::{Engine, text};

impl Engine {
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

    pub(super) fn handle_message(&mut self, kernel: &str, message: &Value) {
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
}

pub(super) fn append_text(previous: &mut Value, incoming: &Value) {
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
