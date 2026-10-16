use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::{Cell, Engine, text};
use crate::notebook;

impl Engine {
    pub(super) fn save(&mut self, params: &Value) -> Result<()> {
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

    pub(super) fn load(&mut self, params: &Value) -> Result<()> {
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
                notebook_id: None,
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
