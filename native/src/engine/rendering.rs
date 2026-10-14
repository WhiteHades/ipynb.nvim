use anyhow::Result;
use base64::Engine as _;
use serde_json::{Value, json};
use std::path::PathBuf;

use super::{Cell, Engine, text};

impl Engine {
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

    pub(super) fn view(&mut self, cell: &Cell) -> Result<Value> {
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
}

fn strip_ansi(text: &str) -> String {
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    ANSI.get_or_init(|| regex::Regex::new(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])").unwrap())
        .replace_all(text, "")
        .into_owned()
}

pub(super) fn render_control_chars(text: &str) -> String {
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
