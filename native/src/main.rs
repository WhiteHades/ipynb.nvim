mod engine;
mod kernel;
mod notebook;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

fn emit(value: &Value) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer(&mut out, value)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let argument = |name: &str| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].clone())
    };
    let python = argument("--python").unwrap_or_else(|| "python3".into());
    let converter = PathBuf::from(argument("--converter").context("--converter is required")?);
    let mut backend = engine::Engine::new(python, &converter)?;
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if !line.trim().is_empty() && send.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        match receive.recv_timeout(Duration::from_millis(10)) {
            Ok(line) => {
                let request: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(error) => {
                        emit(&json!({"id":null,"error":format!("Invalid request: {error}")}))?;
                        continue;
                    }
                };
                let id = &request["id"];
                let Some(method) = request["method"].as_str() else {
                    emit(&json!({"id":id,"error":"Missing method"}))?;
                    continue;
                };
                if method == "shutdown" {
                    emit(&json!({"id":id,"result":null}))?;
                    break;
                }
                let result = backend.dispatch(method, &request["params"]);
                match result {
                    Ok(result) => emit(&json!({"id":id,"result":result}))?,
                    Err(error) => {
                        if let Some(comparison) = error.downcast_ref::<engine::SourceComparison>() {
                            emit(
                                &json!({"id":id,"comparison":comparison.0,"language":comparison.1}),
                            )?;
                        } else {
                            emit(&json!({"id":id,"error":format!("{error:#}")}))?;
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Err(error) = backend.poll() {
            emit(&json!({"event":"error","message":format!("{error:#}")}))?;
        }
        match backend.events() {
            Ok(events) => {
                for event in events {
                    emit(&event)?;
                }
            }
            Err(error) => emit(&json!({"event":"error","message":format!("{error:#}")}))?,
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ipynb-engine: {error:#}");
        std::process::exit(1);
    }
}
