//! A local Neovim E2E check. All assertions live in this Rust driver.
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn notebook(marker: &str) -> Value {
    json!({"nbformat":4,"nbformat_minor":5,"metadata":{
        "kernelspec":{"name":"python3","display_name":"Python 3","language":"python"},
        "language_info":{"name":"python"}, "keep_metadata":"untouched"
    },"cells":[
        {"id":"prose","cell_type":"markdown","metadata":{},"source":"print('KEEP')"},
        {"id":"keep","cell_type":"code","metadata":{},"source":"print('KEEP')",
         "execution_count":1,"outputs":[{"output_type":"stream","name":"stdout","text":"KEEP\n"}]},
        {"id":"run","cell_type":"code","metadata":{},"execution_count":null,"outputs":[],
          "source":format!("import sys, os\nprint(sys.executable)\nprint(os.getcwd())\nimport subprocess\nprint('CHILD=' + subprocess.check_output(['python', '-c', 'import sys; print(sys.executable)'], text=True).strip())\nprint('{marker}')")},
        {"id":"blank","cell_type":"code","metadata":{},"source":"","execution_count":null,"outputs":[]}
    ]})
}

fn project(directory: &Path, environment: &Path, marker: &str) -> PathBuf {
    fs::create_dir_all(directory.join("exercises")).unwrap();
    fs::write(
        directory.join("pyproject.toml"),
        "[project]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(environment, directory.join(".venv")).unwrap();
    let path = directory.join("exercises/work.ipynb");
    fs::write(&path, serde_json::to_vec(&notebook(marker)).unwrap()).unwrap();
    path
}

#[test]
#[ignore = "requires installed Neovim plugins, Pyright, and two Python environments"]
fn project_notebooks_select_isolated_environments_and_recover() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let python = PathBuf::from(std::env::var("IPYNB_TEST_PYTHON").expect("set IPYNB_TEST_PYTHON"));
    let other =
        PathBuf::from(std::env::var("IPYNB_PROJECT_PYTHON").expect("set IPYNB_PROJECT_PYTHON"));
    let plugins = std::env::var("IPYNB_TEST_PLUGINS").expect("set IPYNB_TEST_PLUGINS");
    let temp_root = root.join(".tmp");
    fs::create_dir_all(&temp_root).unwrap();
    let fixture = tempfile::Builder::new()
        .prefix("project-e2e-")
        .tempdir_in(temp_root)
        .unwrap();
    fs::create_dir(fixture.path().join(".git")).unwrap();
    let first = project(
        &fixture.path().join("first project"),
        python.parent().unwrap().parent().unwrap(),
        "PROJECT_ONE",
    );
    let second = project(
        &fixture.path().join("second project"),
        other.parent().unwrap().parent().unwrap(),
        "PROJECT_TWO",
    );
    let shared = first.with_file_name("shared.ipynb");
    fs::write(&shared, serde_json::to_vec(&notebook("SHARED")).unwrap()).unwrap();
    let missing_root = fixture.path().join("missing project");
    fs::create_dir(&missing_root).unwrap();
    fs::write(missing_root.join("uv.lock"), "").unwrap();
    let missing = missing_root.join("work.ipynb");
    fs::write(&missing, serde_json::to_vec(&notebook("MISSING")).unwrap()).unwrap();
    let standalone = fixture.path().join("standalone");
    fs::create_dir(&standalone).unwrap();
    let empty = standalone.join("empty.ipynb");
    fs::write(&empty, "").unwrap();
    let fresh = standalone.join("new.ipynb");
    let stream = standalone.join("stream.ipynb");
    fs::write(
        &stream,
        serde_json::to_vec(&notebook("STREAM_DONE")).unwrap(),
    )
    .unwrap();
    let report = fixture.path().join("report.json");
    let paths = serde_json::to_string(&json!([
        first, second, missing, empty, fresh, stream, shared
    ]))
    .unwrap();
    let script = format!(
        r#"
local result = {{}}
local ok, err = xpcall(function()
  vim.opt.rtp:append(vim.env.IPYNB_TEST_PLUGINS .. '/nvim-lspconfig')
  vim.lsp.config('pyright', {{ cmd = {{ vim.env.IPYNB_PYRIGHT, '--stdio' }} }})
  vim.lsp.enable('pyright')
  local paths = vim.json.decode([==[{paths}]==])
  local native, ui = require('ipynb.native'), require('ipynb.native_ui')
  local function run(buf, marker)
    vim.api.nvim_set_current_buf(buf)
    for row, line in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
      if line == 'import sys, os' then vim.api.nvim_win_set_cursor(0, {{row, 0}}); break end
    end
    vim.cmd([[normal \r]])
    vim.wait(60000, function()
      for _, cell in ipairs(ui.cells(buf)) do
        if not cell.old and cell.status == 'done' and table.concat(cell.lines or {{}}, '\n'):find(marker, 1, true) then return true end
      end
      return false
    end, 20)
    return ui.cells(buf)
  end
  result.projects = {{}}
  local buffers = {{}}
  for index = 1, 2 do
    local opened = vim.uv.hrtime()
    vim.cmd.edit(paths[index])
    local buf = vim.api.nvim_get_current_buf()
    buffers[index] = buf
    local item = {{ preview = ui.cells(buf), before = native.running(buf), open_ms = (vim.uv.hrtime() - opened) / 1e6 }}
    item.cells = run(buf, index == 1 and 'PROJECT_ONE' or 'PROJECT_TWO')
    item.unsaved = vim.bo[buf].modified
    item.running = native.running(buf)
    vim.wait(10000, function()
      local otter = vim.fn.bufnr(paths[index] .. '.otter.py')
      if otter < 0 then return false end
      for _, client in ipairs(vim.lsp.get_clients({{bufnr=otter,name='pyright'}})) do
        item.lsp = {{id=client.id,root=client.config.root_dir,python=(client.settings.python or {{}}).pythonPath}}
        if item.lsp.python then return true end
      end
      return false
    end, 20)
    vim.cmd('IpynbInfo')
    item.info = vim.api.nvim_buf_get_lines(0, 0, -1, false)
    vim.cmd.close()
    vim.cmd.write()
    result.projects[index] = item
  end
  vim.cmd.edit(paths[7])
  local shared_buf = vim.api.nvim_get_current_buf()
  native.call('IpynbInit', {{'shared', native.running(buffers[1])[1]}})
  result.shared_before = native.running(shared_buf)
  vim.api.nvim_set_current_buf(buffers[1])
  vim.cmd('IpynbRestart')
  result.restarted = ui.cells(buffers[1])
  local cell = ui.cells(buffers[1])[2]
  native.request('execute', {{buf=buffers[1],begin=cell.begin,['end']=cell['end'],source="__import__('os')._exit(7)"}})
  vim.wait(10000, function() return #native.running(buffers[1]) == 0 end, 20)
  result.stopped = native.running(buffers[1])
  result.preserved = ui.cells(buffers[1])
  result.shared_stopped = native.running(shared_buf)
  result.recovered = run(buffers[1], 'PROJECT_ONE')
  result.shared_after = native.running(shared_buf)
  result.replacement = native.running(buffers[1])
  result.second_still_running = native.running(buffers[2])
  vim.cmd.write()
  vim.cmd.edit(paths[3])
  local buf = vim.api.nvim_get_current_buf()
  result.missing = require('ipynb.project').resolve(buf, require('ipynb').options)
  result.missing_preview = ui.cells(buf)
  result.missing_running = native.running(buf)
  result.explicit = require('ipynb.project').resolve(buffers[2], {{kernel='ir'}})
  result.standalone = {{}}
  for index = 4, 5 do
    vim.cmd.edit(paths[index])
    local buf = vim.api.nvim_get_current_buf()
    local item = {{ initial = vim.api.nvim_buf_get_lines(buf, 0, -1, false), before = native.running(buf) }}
    if index == 4 then vim.cmd([[normal \b]]) end
    local spans = require('ipynb.cells').spans(buf)
    local span = spans[#spans]
    vim.api.nvim_buf_set_lines(buf, span.begin[1], span['end'][1] + 1, false,
      {{'import sys, os','print(sys.executable)','print(os.getcwd())',"print('STANDALONE')"}})
    item.cells = run(buf, 'STANDALONE')
    vim.wait(10000, function()
      local otter = vim.fn.bufnr(paths[index] .. '.otter.py')
      if otter < 0 then return false end
      for _, client in ipairs(vim.lsp.get_clients({{bufnr=otter,name='pyright'}})) do
        item.lsp = {{root=client.config.root_dir,python=(client.settings.python or {{}}).pythonPath}}
        if item.lsp.python then return true end
      end
      return false
    end, 20)
    vim.cmd.write()
    result.standalone[#result.standalone + 1] = item
  end
  vim.cmd.edit(paths[6])
  local buf = vim.api.nvim_get_current_buf()
  local span = require('ipynb.cells').spans(buf)[2]
  vim.api.nvim_buf_set_lines(buf, span.begin[1], span['end'][1] + 1, false,
    {{'import sys, os',"for _ in range(2000): print('chunkλ', flush=True)","print('STREAM_DONE')"}})
  local streamed = vim.uv.hrtime()
  result.stream = run(buf, 'STREAM_DONE')
  vim.cmd.write()
  result.stream_ms = (vim.uv.hrtime() - streamed) / 1e6
end, debug.traceback)
result.error = not ok and err or nil
vim.fn.writefile({{vim.json.encode(result)}}, vim.env.IPYNB_REPORT)
"#
    );
    let log = fixture.path().join("nvim.log");
    let output = fs::File::create(&log).unwrap();
    let mut child = Command::new("nvim")
        .args(["--headless", "-u"])
        .arg(
            std::env::var_os("IPYNB_TEST_INIT")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("tests/minimal.lua")),
        )
        .arg("-c")
        .arg(format!("lua {script}"))
        .args(["-c", "qa!"])
        .env("IPYNB_TEST_PYTHON", &python)
        .env("IPYNB_TEST_PLUGINS", plugins)
        .env("IPYNB_REPORT", &report)
        .env(
            "IPYNB_PYRIGHT",
            std::env::var("IPYNB_PYRIGHT").expect("set IPYNB_PYRIGHT"),
        )
        .env("XDG_STATE_HOME", fixture.path().join("state"))
        .env("TMPDIR", fixture.path())
        .stdout(Stdio::from(output.try_clone().unwrap()))
        .stderr(Stdio::from(output))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Neovim timed out: {}", fs::read_to_string(log).unwrap());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let log = fs::read_to_string(log).unwrap();
    assert!(status.success(), "Neovim failed: {log}");
    let result: Value = serde_json::from_slice(&fs::read(report).expect(&log)).unwrap();
    assert!(result["error"].is_null(), "{}\n{log}", result["error"]);
    for (index, path) in [first, second].iter().enumerate() {
        let item = &result["projects"][index];
        assert_eq!(item["before"], json!([]), "opening launched a kernel");
        assert!(item["preview"][0]["lines"].to_string().contains("KEEP"));
        assert_eq!(item["preview"][2]["source"], "");
        assert_eq!(item["running"].as_array().unwrap().len(), 1);
        assert_eq!(item["unsaved"], true, "running unchanged code did not mark outputs as unsaved");
        let project = path.parent().unwrap().parent().unwrap();
        let executable = project.join(".venv/bin/python");
        let rendered = item["cells"].to_string();
        assert!(
            rendered.contains(executable.to_str().unwrap()),
            "wrong interpreter: {rendered}"
        );
        assert!(
            rendered.contains(&format!("CHILD={}", executable.display())),
            "subprocess escaped the environment: {rendered}"
        );
        assert!(
            rendered.contains(project.to_str().unwrap()),
            "wrong working directory"
        );
        assert_eq!(
            item["lsp"]["python"],
            json!(executable),
            "wrong LSP interpreter: {result}"
        );
        assert_eq!(item["lsp"]["root"], json!(project));
        assert!(
            item["info"]
                .to_string()
                .contains(executable.to_str().unwrap())
        );
        let saved: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["metadata"]["keep_metadata"], "untouched");
        assert_eq!(saved["cells"][0]["id"], "prose");
        assert_eq!(saved["cells"][1]["id"], "keep");
        assert!(saved["cells"][1]["outputs"].to_string().contains("KEEP"));
        assert_eq!(saved["cells"][3]["id"], "blank");
        println!(
            "Notebook {} opened with saved results in {:.1} ms",
            index + 1,
            item["open_ms"].as_f64().unwrap()
        );
    }
    assert_ne!(
        result["projects"][0]["lsp"]["id"],
        result["projects"][1]["lsp"]["id"]
    );
    assert_eq!(result["stopped"], json!([]));
    assert_eq!(result["shared_stopped"], json!([]));
    assert_eq!(result["shared_before"], result["projects"][0]["running"]);
    assert_eq!(result["shared_after"], result["replacement"]);
    assert!(result["preserved"][0]["lines"].to_string().contains("KEEP"));
    assert!(
        result["restarted"][1]["lines"]
            .to_string()
            .contains("PROJECT_ONE")
    );
    assert!(
        result["recovered"][1]["lines"]
            .to_string()
            .contains("PROJECT_ONE")
    );
    assert_eq!(result["second_still_running"].as_array().unwrap().len(), 1);
    assert!(
        result["missing"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("uv sync")),
        "missing environment was not detected: {result}"
    );
    assert_eq!(result["missing_running"], json!([]));
    assert!(
        result["missing_preview"][0]["lines"]
            .to_string()
            .contains("KEEP")
    );
    assert_eq!(result["explicit"]["kernel"], "ir");
    assert!(result["explicit"]["python"].is_null());
    for item in result["standalone"].as_array().unwrap() {
        assert_eq!(item["before"], json!([]));
        assert!(
            item["initial"].to_string().contains("```python"),
            "empty file did not open as a notebook"
        );
        assert!(
            item["cells"].to_string().contains("STANDALONE"),
            "first cell did not run: {result}\n{log}"
        );
        assert!(
            item["cells"]
                .to_string()
                .contains(standalone.join(".venv/bin/python").to_str().unwrap()),
            "wrong standalone interpreter: {result}\n{log}"
        );
        assert_eq!(item["lsp"]["root"], json!(standalone));
        assert_eq!(
            item["lsp"]["python"],
            json!(standalone.join(".venv/bin/python"))
        );
    }
    for path in [
        empty,
        fresh,
        standalone.join("pyproject.toml"),
        standalone.join("uv.lock"),
        standalone.join(".venv/bin/python"),
    ] {
        assert!(
            path.is_file(),
            "local notebook file missing: {}",
            path.display()
        );
    }
    let saved: Value = serde_json::from_slice(&fs::read(stream).unwrap()).unwrap();
    let output: String = saved["cells"][2]["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|output| {
            output["text"]
                .as_array()
                .unwrap()
                .iter()
                .map(|line| line.as_str().unwrap())
        })
        .collect();
    assert_eq!(output, format!("{}STREAM_DONE\n", "chunkλ\n".repeat(2000)));
    assert_eq!(result["stream"][1]["lines"].as_array().unwrap().len(), 2001);
    println!(
        "Streaming 2000 Unicode lines completed and saved in {:.1} ms",
        result["stream_ms"].as_f64().unwrap()
    );
}
