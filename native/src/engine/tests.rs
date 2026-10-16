use super::matching::comparison_sources;
use super::outputs::append_text;
use super::persistence::imported_status;
use super::rendering::render_control_chars;
use super::*;
use crate::notebook;
use std::time::Instant;

fn cell(kernel: &str, source: &str) -> Cell {
    Cell {
        id: 1,
        buf: 1,
        kernel: kernel.into(),
        begin: [0, 0],
        end: [0, source.len()],
        source: source.into(),
        notebook_id: None,
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
            json!({"argv":["/usr/bin/true"],"language":"r"}).to_string(),
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
        "python":"/usr/bin/true","replace":"test"}),
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
        "python":"/usr/bin/true","replace":replacement}),
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
        engine.sync_positions(&json!({"positions":[
            {"id":"2","source":"ignored"},
            {"id":99,"source":"unknown"},
            {"source":"missing"},
            {"id":2,"begin":[1,0],"end":[1,3],"source":"new"},
            {"id":2,"begin":[2,0],"end":[2,3]}
        ]}));
        assert!(!engine.view_cache.contains_key(&2));
        assert_eq!(engine.cells[0].source, "one");
        assert_eq!(engine.cells[1].source, "new");
        assert_eq!(engine.cells[1].begin, [2, 0]);
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

#[test]
fn misplaced_jupytext_markdown_recovers_without_losing_source() -> Result<()> {
    let directory = save_directory()?;
    let path = directory.path().join("recovered.ipynb");
    let source = "---\njupyter:\n  jupytext:\n    text_representation:\n      extension: .md\n      format_name: markdown\n---\n\n# Notes\n\n```python\nprint(42)\n```\n";
    std::fs::write(&path, source)?;
    let mut engine = save_engine()?;
    let readonly = engine.read(&json!({"path":path,"readonly":true}))?;
    assert_eq!(std::fs::read_to_string(&path)?, source);
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    assert_eq!(readonly["recovery_pending"], true);
    let recovered = engine.read(&json!({"path":path}))?;
    assert_eq!(
        std::fs::read_to_string(recovered["recovery_backup"].as_str().unwrap())?,
        source
    );
    assert_eq!(recovered["text"], readonly["text"]);
    let notebook = notebook::read_json(&path)?;
    assert_eq!(notebook["cells"].as_array().unwrap().len(), 2);
    assert_eq!(text(&notebook["cells"][1]["source"]), "print(42)");
    let lines: Vec<_> = recovered["text"].as_str().unwrap().split('\n').collect();
    let row = lines.iter().position(|line| *line == "```python").unwrap() + 1;
    engine.import(&json!({"path":path,"buf":1,"preview":true,"lines":lines,
        "spans":[{"begin":[row,0],"end":[row,lines[row].len()]}]}))?;
    assert_eq!(engine.cells.len(), 1);
    engine.write(&json!({"path":path,"buf":1,"text":recovered["text"],
        "mtime":notebook::file_mtime(&path)?}))?;
    assert_eq!(notebook::read_json(&path)?["cells"], notebook["cells"]);
    for invalid in [
        "{broken",
        "# ordinary markdown",
        "---\njupyter:\n  kernelspec: {}\n---\n",
    ] {
        std::fs::write(&path, invalid)?;
        assert!(engine.read(&json!({"path":path})).is_err());
        assert_eq!(std::fs::read_to_string(&path)?, invalid);
    }
    Ok(())
}

#[test]
fn initial_preview_reuses_verified_conversion_without_guessing_spans() -> Result<()> {
    let directory = save_directory()?;
    let path = directory.path().join("cached.ipynb");
    let notebook = json!({"nbformat":4,"nbformat_minor":5,"metadata":{},"cells":[
        {"id":"one","cell_type":"code","metadata":{},"source":"print(1)","execution_count":1,
         "outputs":[{"output_type":"stream","name":"stdout","text":"one\n"}]},
        {"id":"example","cell_type":"markdown","metadata":{},"source":"An example:\n\n```python\n\n```"},
        {"id":"empty","cell_type":"code","metadata":{},"source":"","execution_count":2,"outputs":[]},
        {"id":"two","cell_type":"code","metadata":{},"source":"print(1)","execution_count":3,
         "outputs":[{"output_type":"stream","name":"stdout","text":"two\n"}]}]});
    notebook::write_atomic(&path, &notebook, None)?;
    let original = std::fs::read(&path)?;
    let mut engine = save_engine()?;
    let result = engine.read(&json!({"path":path,"template":""}))?;
    let lines: Vec<_> = result["text"].as_str().unwrap().split('\n').collect();
    let spans: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("```python"))
        .map(|(row, _)| json!({"begin":[row+1,0],"end":[row+1,lines[row+1].len()]}))
        .collect();
    assert_eq!(spans.len(), 4);
    engine.converter = Converter::new("/missing/ipynb/python", Path::new("unused"));
    let params = json!({"path":path,"buf":1,"preview":true,"lines":lines,"spans":spans});
    engine.import(&params)?;
    assert_eq!(engine.cells.len(), 3);
    assert_eq!(text(&engine.cells[0].outputs[0]["text"]), "one\n");
    assert_eq!(text(&engine.cells[2].outputs[0]["text"]), "two\n");
    // Once consumed, or if the buffer changed, use Jupytext's full span resolution.
    assert!(engine.import(&params).is_err());
    let mut engine = save_engine()?;
    engine.read(&json!({"path":path,"template":""}))?;
    engine.converter = Converter::new("/missing/ipynb/python", Path::new("unused"));
    let mut changed = params.clone();
    changed["lines"][0] = json!("changed");
    assert!(engine.import(&changed).is_err());
    assert_eq!(std::fs::read(&path)?, original);
    Ok(())
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

#[test]
fn jupytext_config_pair_sync_and_export_keep_outputs_and_reject_conflicts() -> Result<()> {
    let directory = save_directory()?;
    let path = directory.path().join("paired.ipynb");
    let script = directory.path().join("paired.py");
    std::fs::write(
        directory.path().join("jupytext.toml"),
        "formats = 'ipynb,py:percent'\ncell_metadata_filter = 'all'\n",
    )?;
    let mut engine = save_engine()?;
    let config = directory.path().join("jupytext.toml");
    let settings = std::fs::read_to_string(&config)?;
    let modified = std::fs::metadata(&config)?.modified()?;
    let source = "# First\n\n# Second\n\n```python\nprint(1)\n```";
    let parsed = engine.converter.convert_at(source, None, Some(&path))?;
    assert_eq!(parsed["cells"].as_array().unwrap().len(), 2);
    std::fs::write(&config, format!("{settings}split_at_heading = true\n"))?;
    std::fs::File::options()
        .write(true)
        .open(&config)?
        .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    assert_eq!(
        engine.converter.convert_at(source, None, Some(&path))?["cells"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    std::fs::remove_file(&config)?;
    assert_eq!(
        engine.converter.convert_at(source, None, Some(&path))?["cells"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    std::fs::write(&config, "split_at_heading = [invalid")?;
    assert!(
        engine
            .converter
            .convert_at(source, None, Some(&path))
            .is_err()
    );
    std::fs::write(&config, &settings)?;
    let mut existing = parsed.clone();
    existing["cells"][1]["outputs"] =
        json!([{"output_type":"stream","name":"stdout","text":"kept\n"}]);
    existing["cells"][1]["execution_count"] = json!(7);
    let merged = engine
        .converter
        .convert_at(source, Some(existing), Some(&path))?;
    assert_eq!(text(&merged["cells"][1]["outputs"][0]["text"]), "kept\n");
    assert_eq!(merged["cells"][1]["execution_count"], 7);
    assert_eq!(
        engine.converter.convert_at(source, None, Some(&path))?["cells"][1]["outputs"],
        json!([])
    );
    let (buffer, mtime) = save_duplicate_fixture(&mut engine, &path)?;
    let read = engine.read(&json!({"path":path}))?;
    assert!(text(&read["text"]).contains("custom="));
    let mut params = json!({"path":path,"buf":1,"text":buffer,"mtime":mtime});
    let saved = engine.write(&params)?;
    assert!(std::fs::read_to_string(&script)?.contains("# %%"));
    assert_eq!(
        text(&notebook::read_json(&path)?["cells"][2]["outputs"][0]["text"]),
        "new\n"
    );
    params["mtime"] = saved["mtime"].clone();

    let compared = engine.converter.read(&path, Path::new(""))?;
    let original = std::fs::read(&path)?;
    let external =
        std::fs::read_to_string(&script)? + "\n# %% [markdown]\n# Edited outside Neovim\n";
    std::fs::write(&script, &external)?;
    assert!(
        engine
            .remember_pairs(&path, &compared)
            .unwrap_err()
            .to_string()
            .contains("changed on disk")
    );
    let error = engine.write(&params).unwrap_err();
    assert!(
        error.to_string().contains("Paired files changed"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(&path)?, original);
    assert_eq!(std::fs::read_to_string(&script)?, external);
    params["action"] = json!("sync");
    params["source"] = json!(script);
    let synced = engine.jupytext(&params)?;
    assert!(text(&synced["text"]).contains("Edited outside Neovim"));
    let notebook = notebook::read_json(&path)?;
    assert_eq!(notebook["metadata"]["custom"], "keep");
    assert_eq!(notebook["cells"][0]["id"], "prose");
    assert!(notebook["cells"][0]["attachments"]["tiny.png"].is_object());
    assert_eq!(text(&notebook["cells"][2]["outputs"][0]["text"]), "new\n");
    params["text"] = synced["text"].clone();
    params["mtime"] = synced["mtime"].clone();

    let compatible = std::fs::read_to_string(&script)?;
    let incompatible = compatible
        .lines()
        .map(|line| {
            if line.contains("format_version:") {
                "#       format_version: '99.0'"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_ne!(compatible.trim_end(), incompatible.trim_end());
    std::fs::write(&script, &incompatible)?;
    let before_sync = std::fs::read(&path)?;
    assert!(
        engine
            .jupytext(&params)
            .unwrap_err()
            .to_string()
            .contains("JupytextFormatError")
    );
    assert_eq!(std::fs::read(&path)?, before_sync);
    assert_eq!(std::fs::read_to_string(&script)?, incompatible);
    std::fs::write(&script, compatible)?;
    let restored = engine.jupytext(&params)?;
    params["text"] = restored["text"].clone();
    params["mtime"] = restored["mtime"].clone();

    params["action"] = json!("info");
    let info = engine.jupytext(&params)?;
    assert!(
        info["formats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["format"] == "md:myst")
    );
    assert!(info["config"].as_str().unwrap().ends_with("jupytext.toml"));
    let target = directory.path().join("export.md");
    params["action"] = json!("export");
    params["fmt"] = json!("md:markdown");
    params["target"] = json!(target);
    engine.jupytext(&params)?;
    let exported = std::fs::read(&target)?;
    assert!(engine.jupytext(&params).is_err());
    assert_eq!(std::fs::read(&target)?, exported);

    params["action"] = json!("check");
    params["command"] = json!("true");
    engine.jupytext(&params)?;
    params["action"] = json!("pipe");
    params["command"] = json!("cat");
    let proposed = engine.jupytext(&params)?;
    assert!(text(&proposed["text"]).contains("Edited outside Neovim"));
    assert!(proposed["writes"].as_array().unwrap().is_empty());
    params["command"] = json!("false");
    assert!(engine.jupytext(&params).is_err());

    params["action"] = json!("unpair");
    let unpaired = engine.jupytext(&params)?;
    assert_eq!(
        notebook::read_json(&path)?["metadata"]["jupytext"]["formats"],
        "ipynb"
    );
    assert!(script.is_file());
    assert!(unpaired["mtime"].is_object());
    let fresh_path = directory.path().join("new.ipynb");
    let fresh = engine.read(&json!({"path":fresh_path,
        "template":Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("data/template.ipynb")}))?;
    engine.write(&json!({"path":fresh_path,"buf":2,"text":fresh["text"],"mtime":null}))?;
    assert!(fresh_path.with_extension("py").is_file());
    let reopened = engine.read(&json!({"path":fresh_path}))?;
    assert_eq!(reopened["paired_changed"], json!([]));
    std::fs::remove_file(directory.path().join("jupytext.toml"))?;
    let marker = directory.path().join("executed");
    std::fs::write(
        directory.path().join(".jupytext.py"),
        format!("open({:?}, 'w').write('unsafe')", marker),
    )?;
    assert!(
        engine
            .read(&json!({"path":path}))
            .unwrap_err()
            .to_string()
            .contains("Executable Jupytext config")
    );
    assert!(!marker.exists());
    Ok(())
}

#[test]
fn output_export_rejects_external_inputs_even_when_the_mtime_is_preserved() -> Result<()> {
    let directory = save_directory()?;
    let path = directory.path().join("external.ipynb");
    let mut engine = save_engine()?;
    let (buffer, mtime) = save_duplicate_fixture(&mut engine, &path)?;
    engine.read(&json!({"path":path}))?;
    let modified = std::fs::metadata(&path)?.modified()?;
    let external = std::fs::read_to_string(&path)?.replacen("print(1)", "print(9)", 1);
    std::fs::write(&path, &external)?;
    std::fs::File::options()
        .write(true)
        .open(&path)?
        .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    assert_eq!(
        notebook::file_mtime(&path)?,
        Some(serde_json::from_value(mtime.clone())?)
    );
    let params = json!({"path":path,"buf":1,"overwrite":true,"mtime":mtime,
        "lines":buffer.split('\n').collect::<Vec<_>>()});
    assert!(
        engine
            .export(&params)
            .unwrap_err()
            .to_string()
            .contains("changed on disk")
    );
    assert_eq!(std::fs::read_to_string(&path)?, external);
    assert!(
        engine
            .write(&json!({"path":path,"buf":1,"text":buffer,"mtime":mtime}))
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path)?, external);
    Ok(())
}

#[cfg(unix)]
#[test]
fn opening_optional_pairs_does_not_run_external_converters() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let directory = save_directory()?;
    let bin = directory.path().join("bin");
    std::fs::create_dir(&bin)?;
    let marker = directory.path().join("external-ran");
    let executable = bin.join("quarto");
    std::fs::write(
        &executable,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
    let pandoc = bin.join("pandoc");
    std::fs::copy(&executable, &pandoc)?;
    let wrapper = directory.path().join("python");
    let python = std::env::var("IPYNB_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport PATH='{}':\"$PATH\"\nexec '{}' \"$@\"\n",
            bin.display(),
            python.replace('\'', "'\\''")
        ),
    )?;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))?;
    let path = directory.path().join("optional.ipynb");
    notebook::write_atomic(
        &path,
        &json!({"nbformat":4,"nbformat_minor":5,"cells":[],
        "metadata":{"jupytext":{"formats":"ipynb,qmd"}}}),
        None,
    )?;
    let paired = path.with_extension("qmd");
    std::fs::write(&paired, "---\ntitle: Notebook\n---\n")?;
    let mut engine = Engine::new(
        wrapper.to_string_lossy().into_owned(),
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("converter.py"),
    )?;
    let read = engine.read(&json!({"path":path}))?;
    assert_eq!(read["paired_changed"], json!([paired]));
    assert!(
        read["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("---\n"))
    );
    assert!(!marker.exists());
    let info =
        engine.jupytext(&json!({"action":"info","path":path,"buf":1,"text":read["text"]}))?;
    assert!(info["formats"].as_array().is_some_and(|formats| {
        formats
            .iter()
            .any(|format| format["format"] == "md:markdown")
    }));
    assert!(
        marker.exists(),
        "explicit format discovery must retain access to optional tools"
    );
    Ok(())
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
    let mut positioned = save_engine()?;
    let prefix = positioned.converter.marker_prefix("").to_owned();
    let collision = format!("```python\nprint({prefix:?})\n```");
    let replacement = positioned.converter.marker_prefix(&collision).to_owned();
    assert_ne!(replacement, prefix);
    assert!(!collision.contains(&replacement));
    assert_eq!(positioned.converter.marker_prefix(&collision), replacement);
    let source = "```python\n# 日本語\nprint(1)\n```\n\n```python\nprint(1)\n```";
    let (sources, positions) = super::matching::positioned_sources(
        &mut positioned.converter,
        source,
        &[
            (3, [6, 0]),
            (2, [2, 0]),
            (4, [2, 0]),
            (2, [2, 0]),
            (1, [1, 2]),
        ],
        None,
    )?;
    assert_eq!(sources, ["# 日本語\nprint(1)", "print(1)"]);
    assert_eq!(positions, HashMap::from([(1, 0), (2, 0), (3, 1), (4, 0)]));
    let (sources, positions) = super::matching::positioned_sources(
        &mut positioned.converter,
        &format!("{source}\n"),
        &[(5, [8, 0])],
        None,
    )?;
    assert_eq!(sources, ["# 日本語\nprint(1)", "print(1)"]);
    assert!(positions.is_empty());
    let (sources, positions) = super::matching::positioned_sources(
        &mut positioned.converter,
        "```python\n```\n\n```python\nprint(2)\n```",
        &[(1, [1, 0]), (2, [4, 0])],
        None,
    )?;
    assert_eq!(sources, ["", "print(2)"]);
    assert_eq!(positions, HashMap::from([(1, 0), (2, 1)]));
    for starts in [vec![(1, [1, 3])], vec![(1, [99, 0])], vec![(1, [1, 99])]] {
        assert!(
            super::matching::positioned_sources(&mut positioned.converter, source, &starts, None)
                .is_err()
        );
    }
    assert!(
        super::matching::positioned_sources(
            &mut positioned.converter,
            source,
            &[(7, [2, 0]), (7, [6, 0])],
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("Ambiguous notebook output position")
    );
    drop(positioned);
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
        let directory = save_directory()?;
        let path = directory.path().join("emptied.ipynb");
        let mut engine = save_engine()?;
        let buffer = "```python\nprint(1)\n```\n\n```python\n\n```\n\n```python\n\n```";
        let mut original = engine.converter.convert(buffer, None)?;
        original["metadata"]["jupytext"]["cell_metadata_filter"] = json!("-all");
        for (index, cell) in original["cells"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            cell["id"] = json!(format!("cell-{index}"));
            cell["metadata"] = json!({"private":index,"tags":[format!("tag-{index}")]});
            cell["execution_count"] = json!(index + 1);
            cell["outputs"] =
                json!([{"output_type":"stream","name":"stdout","text":format!("{index}\n")}]);
        }
        notebook::split_lines(&mut original);
        notebook::write_atomic(&path, &original, None)?;
        engine.import(&json!({"buf":1,"path":path,"lines":buffer.split('\n').collect::<Vec<_>>(),
            "spans":[{"begin":[1,0],"end":[1,8]},{"begin":[5,0],"end":[5,0]},{"begin":[9,0],"end":[9,0]}]}))?;
        let edited = buffer.replacen("print(1)\n", "", 1);
        for (cell, row) in engine.cells.iter_mut().zip([1, 4, 8]) {
            cell.begin = [row, 0];
            cell.end = [row, 0];
            cell.source.clear();
        }
        for _ in 0..2 {
            engine.write(&json!({"buf":1,"path":path,"text":edited}))?;
            let saved = notebook::read_json(&path)?;
            for (index, cell) in saved["cells"].as_array().unwrap().iter().enumerate() {
                assert_eq!(text(&cell["source"]), "");
                for key in ["id", "metadata", "execution_count", "outputs"] {
                    assert_eq!(
                        cell[key], original["cells"][index][key],
                        "cell {index} {key}"
                    );
                }
            }
        }
    }
    {
        let mut engine = save_engine()?;
        let buffer =
            "```python\n\n```\n\n```python\nsecond = 2\n```\n\n```python\nfirst = 1 # live\n```";
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
        for (row, source, count) in [(1, "", 1), (5, "second = 2", 2), (9, "first = 1 # live", 3)] {
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
            .merge_outputs(&mut notebook, 1, None, &json!({}), Some(buffer), None)
            .unwrap_err();
        assert!(comparison.downcast_ref::<SourceComparison>().is_some());
        assert_eq!(notebook, disk);
        assert!(engine.merge_outputs(&mut notebook, 1, None, &normalized, Some(buffer), None)?);
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
            .merge_outputs(&mut notebook, 1, None, &json!({}), None, None)
            .unwrap_err();
        assert!(error.to_string().contains("Ambiguous"));
        assert_eq!(notebook, original);
        let buffer = "```python\nprint(1) # new\n```\n\n```python\nprint(1) # new\n```";
        engine.cells[0].begin = [5, 0];
        engine.cells[0].source = "print(1) # new".into();
        let normalized = json!({"python":{"print(1)":"print(1)\n","print(1) # new":"print(1)\n"}});
        let error = engine
            .merge_outputs(&mut notebook, 1, None, &normalized, Some(buffer), None)
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
            .merge_outputs(&mut prose_only, 1, None, &json!({}), Some("print(1)"), None)
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
        let buffer =
            "print(1)\n\n```python\nprint(1)\n```\n\n```python\n\n```\n\n```python\nprint(1)\n```";
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
        assert!(engine.merge_outputs(&mut notebook, 1, None, &normalized, Some(buffer), None)?);
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
