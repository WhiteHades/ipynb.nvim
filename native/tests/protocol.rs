//! One real kernel lifecycle through the engine's public JSON protocol.
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

struct Engine {
    child: Child,
    input: ChildStdin,
    messages: Receiver<Value>,
    id: u64,
    cells: HashMap<u64, Value>,
}

impl Engine {
    fn send(&mut self, method: &str, params: Value) -> u64 {
        self.id += 1;
        serde_json::to_writer(
            &mut self.input,
            &json!({"id":self.id,"method":method,"params":params}),
        )
        .unwrap();
        writeln!(self.input).unwrap();
        self.input.flush().unwrap();
        self.id
    }

    fn receive(&mut self) -> Value {
        let message = self
            .messages
            .recv_timeout(Duration::from_secs(30))
            .expect("engine response timed out");
        assert_ne!(message["event"], "error", "{message}");
        if message["event"] == "cells" {
            self.cells
                .insert(message["buf"].as_u64().unwrap(), message["cells"].clone());
        }
        if message["event"] == "input" {
            self.send(
                "stdin",
                json!({"kernel":message["kernel"],"value":"answer"}),
            );
        }
        message
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        loop {
            let message = self.receive();
            if message["id"] == id {
                return message;
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let response = self.request(method, params);
        assert!(
            response.get("error").is_none() && response.get("comparison").is_none(),
            "{response}"
        );
        response["result"].clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
        }
    }
}

#[test]
fn execution_stdin_shared_detach_and_saved_state_roundtrip() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_ipynb-engine"))
        .arg("--python")
        .arg(std::env::var("IPYNB_TEST_PYTHON").unwrap_or_else(|_| "python3".into()))
        .arg("--converter")
        .arg(root.join("converter.py"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (sender, messages) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let value = serde_json::from_str(&line.unwrap()).unwrap();
            if sender.send(value).is_err() {
                break;
            }
        }
    });
    let mut engine = Engine {
        child,
        input,
        messages,
        id: 0,
        cells: HashMap::new(),
    };
    engine.call(
        "configure",
        json!({"image_provider":"none","numpy_legacy_repr":false}),
    );
    let kernel = engine.call("init", json!({"buf":1,"kernel":"python3"}));
    engine.call("init", json!({"buf":2,"kernel":kernel,"shared":true}));
    let source = "from IPython.display import display\nprint(input('value: '))\ndisplay({'text/plain': 'rich', 'application/x-test': {'kept': True}}, raw=True)";
    let lines: Vec<_> = source.split('\n').collect();
    let cell = engine.call(
        "execute",
        json!({"buf":1,"kernel":kernel,"begin":[0,0],
        "end":[lines.len()-1,lines.last().unwrap().len()],"source":source}),
    );
    let started = Instant::now();
    while !engine
        .cells
        .get(&1)
        .and_then(Value::as_array)
        .is_some_and(|cells| {
            cells
                .iter()
                .any(|c| c["id"] == cell && c["status"] == "done")
        })
    {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "execution did not finish"
        );
        engine.receive();
    }
    let base = root.parent().unwrap().join(".tmp");
    std::fs::create_dir_all(&base).unwrap();
    let directory = tempfile::tempdir_in(base).unwrap();
    let path = directory.path().join("state.json");
    engine.call(
        "save",
        json!({"buf":1,"kernel":kernel,"path":path,"lines":lines}),
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["cells"][0]["chunks"][0]["data"]["text/plain"],
        "answer\n"
    );
    assert_eq!(
        saved["cells"][0]["chunks"][1]["data"]["application/x-test"]["kept"],
        true
    );
    engine.call("deinit", json!({"buf":1}));
    assert_eq!(engine.call("state", json!({}))["kernels"], json!([kernel]));
    engine.call("deinit", json!({"buf":2}));
    assert_eq!(engine.call("state", json!({}))["kernels"], json!([]));
    let failure = engine.request("load", json!({"buf":1,"path":path,"lines":["changed"]}));
    assert!(
        failure.get("error").is_some() || failure.get("comparison").is_some(),
        "{failure}"
    );
    assert_eq!(engine.call("state", json!({}))["kernels"], json!([]));
    engine.call("load", json!({"buf":1,"path":path,"lines":lines}));
    engine.call("save", json!({"buf":1,"path":path,"lines":lines}));
    let roundtrip: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(roundtrip, saved);
    engine.call("shutdown", json!({}));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = engine.child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "engine shutdown timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    reader.join().unwrap();
}
