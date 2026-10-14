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
    let paired_script = first.with_extension("py");
    let exported_markdown = first.with_file_name("jupytext-export.md");
    let report = fixture.path().join("report.json");
    let paths = serde_json::to_string(&json!([
        first, second, missing, empty, fresh, stream, shared
    ]))
    .unwrap();
    let script = format!(
        r#"
local result = {{}}
local original_input, original_select, original_notify = vim.ui.input, vim.ui.select, vim.notify
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
    vim.cmd('IpynbEnterOutput')
    item.output = {{ opened = vim.api.nvim_get_current_buf() ~= buf,
      text = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '\n') }}
    vim.cmd('IpynbHideOutput')
    item.output.closed = vim.api.nvim_get_current_buf() == buf
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
  local first_buf = buffers[1]
  vim.api.nvim_set_current_buf(first_buf)
  local paired_script = vim.fn.fnamemodify(paths[1], ':r') .. '.py'
  local export_path = vim.fn.fnamemodify(paths[1], ':h') .. '/jupytext-export.md'
  local function snapshot()
    vim.wait(10000, function() return #ui.cells(first_buf) >= 3 end, 20)
    return {{ buffer = table.concat(vim.api.nvim_buf_get_lines(first_buf, 0, -1, false), '\n'),
      mtime = vim.b[first_buf].mtime, disk_mtime = vim.uv.fs_stat(paths[1]).mtime,
      cells = ui.cells(first_buf), kernel = native.running(first_buf), modified = vim.bo[first_buf].modified }}
  end
  local item = {{ before = snapshot() }}
  result.jupytext = item
  item.menu_cancelled = {{}}
  for _, reason in ipairs({{'focus', 'changed'}}) do
    vim.api.nvim_set_current_buf(first_buf)
    local observation = {{ notices = {{}}, selections = 0 }}
    item.menu_cancelled[reason] = observation
    local pending, selected
    vim.ui.select = function(items, _, callback)
      observation.selections = observation.selections + 1
      pending = callback
      for _, candidate in ipairs(items) do
        if type(candidate) == 'table' and candidate[1] == 'Jupytext formats and sync' then selected = candidate end
      end
    end
    vim.notify = function(message) observation.notices[#observation.notices + 1] = tostring(message) end
    vim.cmd('Ipynb')
    if reason == 'focus' then
      vim.api.nvim_set_current_buf(buffers[2])
    else
      vim.api.nvim_buf_set_lines(first_buf, -1, -1, false, {{'', 'MENU_CHANGED'}})
    end
    pending(selected)
    observation.current = vim.api.nvim_get_current_buf()
    observation.expected_current = reason == 'focus' and buffers[2] or first_buf
    vim.notify = original_notify
  end
  vim.api.nvim_set_current_buf(first_buf)
  local answer, choice = 'ipynb,py:percent', 'Pair without overwriting existing files'
  vim.ui.input = function(_, callback) callback(answer) end
  vim.ui.select = function(items, _, callback)
    for _, candidate in ipairs(items) do
      if candidate == choice or type(candidate) == 'table' and (candidate.source == choice or candidate.format == choice) then
        callback(candidate); return
      end
    end
    callback(nil)
  end
  vim.cmd('IpynbJupytext pair')
  item.paired = snapshot()
  item.pair_text = table.concat(vim.fn.readfile(paired_script, 'b'), '\n')
  vim.fn.writefile({{'', '# %% [markdown]', '# EXTERNAL_JUPYTEXT_MARKDOWN'}}, paired_script, 'a')
  local rejected = {{ notebook_before = vim.fn.readfile(paths[1], 'b'), pair_before = vim.fn.readfile(paired_script, 'b'), notices = {{}} }}
  item.rejected = rejected
  vim.notify = function(message, level) rejected.notices[#rejected.notices + 1] = {{ message = tostring(message), level = level }} end
  rejected.write_ok, rejected.error = pcall(vim.cmd.write)
  vim.notify = original_notify
  rejected.notebook_after, rejected.pair_after = vim.fn.readfile(paths[1], 'b'), vim.fn.readfile(paired_script, 'b')
  choice = paired_script
  vim.cmd('IpynbJupytext sync')
  item.synced = snapshot()
  for _, cell in ipairs(ui.cells(first_buf)) do
    if cell.source:find('PROJECT_ONE', 1, true) then
      vim.api.nvim_win_set_cursor(0, {{cell.begin[1] + 1, 0}})
      item.reevaluate_ok, item.reevaluate_error = pcall(native.call, 'IpynbReevaluateCell')
      vim.wait(60000, function()
        for _, updated in ipairs(ui.cells(first_buf)) do
          if updated.id == cell.id and not updated.old and updated.status == 'done' then return true end
        end
        return false
      end, 20)
      item.reevaluated = snapshot()
      break
    end
  end
  item.save_ok, item.save_error = pcall(vim.cmd.write)
  item.saved = snapshot()
  answer, choice = export_path, 'md:markdown'
  vim.cmd('IpynbJupytext export')
  item.exported = snapshot()
  item.pipe_disk_before = vim.fn.readfile(paths[1], 'b')
  answer = "sed 's/^# print(/# PIPE_REVIEWED print(/'"
  local picker = vim.ui.select
  vim.ui.select = function(items, opts, callback)
    if opts.prompt == 'Execute external command: ' .. answer then
      callback('Run command')
    elseif opts.prompt == 'Apply the reviewed pipe changes?' then
      item.pipe_review = {{ filetype = vim.bo.filetype,
        text = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '\n') }}
      callback('Apply to editor buffer')
    else
      picker(items, opts, callback)
    end
  end
  vim.cmd('IpynbJupytext pipe')
  vim.ui.select = picker
  item.piped = snapshot()
  item.pipe_disk_after = vim.fn.readfile(paths[1], 'b')
  item.pipe_save_ok, item.pipe_save_error = pcall(vim.cmd.write)
  item.pipe_saved = snapshot()
end, debug.traceback)
vim.ui.input, vim.ui.select, vim.notify = original_input, original_select, original_notify
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
        assert_eq!(item["output"]["opened"], true, "full output did not open");
        assert!(
            item["output"]["text"]
                .as_str()
                .unwrap()
                .contains(if index == 0 {
                    "PROJECT_ONE"
                } else {
                    "PROJECT_TWO"
                })
        );
        assert_eq!(
            item["output"]["closed"], true,
            "full output did not return focus"
        );
        assert_eq!(
            item["unsaved"], true,
            "running unchanged code did not mark outputs as unsaved"
        );
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
    let jupytext = &result["jupytext"];
    assert_eq!(jupytext["before"]["kernel"], result["replacement"]);
    for phase in ["paired", "synced", "saved", "exported", "pipe_saved"] {
        let state = &jupytext[phase];
        assert_eq!(
            state["kernel"], jupytext["before"]["kernel"],
            "{phase} changed the kernel"
        );
        assert_eq!(state["modified"], false, "{phase} left an unsaved buffer");
        assert_eq!(
            state["mtime"], state["disk_mtime"],
            "{phase} has a stale mtime"
        );
        let outputs: String = state["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["lines"].to_string())
            .collect();
        assert!(
            outputs.contains("KEEP") && outputs.contains("PROJECT_ONE"),
            "{phase} lost output: {state}"
        );
        for cell in state["cells"].as_array().unwrap() {
            assert_eq!(
                cell["kernel"], state["kernel"][0],
                "{phase} restored a cell with the wrong kernel: {cell}"
            );
        }
    }
    for reason in ["focus", "changed"] {
        let cancelled = &jupytext["menu_cancelled"][reason];
        assert_eq!(
            cancelled["selections"], 1,
            "stale menu opened Jupytext: {cancelled}"
        );
        assert_eq!(cancelled["current"], cancelled["expected_current"]);
        assert!(
            cancelled["notices"].to_string().contains("cancelled"),
            "missing menu cancellation: {cancelled}"
        );
    }
    assert_eq!(
        jupytext["reevaluate_ok"], true,
        "reevaluation failed: {}",
        jupytext["reevaluate_error"]
    );
    assert!(
        jupytext["reevaluated"]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cell| {
                cell["source"]
                    .as_str()
                    .unwrap_or("")
                    .contains("PROJECT_ONE")
                    && cell["old"] == false
                    && cell["status"] == "done"
                    && cell["lines"].to_string().contains("PROJECT_ONE")
            }),
        "restored cell was not reevaluated: {}",
        jupytext["reevaluated"]
    );
    assert_eq!(jupytext["pipe_review"]["filetype"], "diff");
    assert!(
        jupytext["pipe_review"]["text"]
            .as_str()
            .unwrap()
            .contains("PIPE_REVIEWED")
    );
    assert_eq!(
        jupytext["piped"]["modified"], true,
        "pipe changes were not marked unsaved"
    );
    assert_eq!(
        jupytext["pipe_disk_before"], jupytext["pipe_disk_after"],
        "pipe application changed the notebook on disk"
    );
    assert_eq!(
        jupytext["pipe_save_ok"], true,
        "pipe save failed: {}",
        jupytext["pipe_save_error"]
    );
    assert!(
        jupytext["pipe_saved"]["buffer"]
            .as_str()
            .unwrap()
            .contains("PIPE_REVIEWED")
    );
    assert_eq!(
        jupytext["piped"]["buffer"],
        jupytext["pipe_saved"]["buffer"]
    );
    assert!(jupytext["pair_text"].as_str().unwrap().contains("# %%"));
    let rejected = &jupytext["rejected"];
    assert_eq!(
        rejected["write_ok"], false,
        "write accepted a changed pair: {rejected}"
    );
    assert!(
        rejected.to_string().contains("Paired files changed"),
        "wrong write failure: {rejected}"
    );
    assert_eq!(
        rejected["notebook_before"], rejected["notebook_after"],
        "rejected write changed the notebook"
    );
    assert_eq!(
        rejected["pair_before"], rejected["pair_after"],
        "rejected write changed the script"
    );
    assert!(
        jupytext["synced"]["buffer"]
            .as_str()
            .unwrap()
            .contains("EXTERNAL_JUPYTEXT_MARKDOWN")
    );
    assert_eq!(
        jupytext["save_ok"], true,
        "save failed after sync: {}",
        jupytext["save_error"]
    );
    assert_eq!(jupytext["synced"]["buffer"], jupytext["saved"]["buffer"]);
    assert_eq!(
        jupytext["saved"]["buffer"], jupytext["exported"]["buffer"],
        "export changed the editor"
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(paired_script.with_extension("ipynb")).unwrap()).unwrap();
    assert_eq!(saved["metadata"]["jupytext"]["formats"], "ipynb,py:percent");
    assert!(
        saved["cells"][0]["source"]
            .to_string()
            .contains("PIPE_REVIEWED")
    );
    assert!(saved["cells"][1]["outputs"].to_string().contains("KEEP"));
    assert!(
        saved["cells"][2]["outputs"]
            .to_string()
            .contains("PROJECT_ONE")
    );
    assert!(
        fs::read_to_string(paired_script)
            .unwrap()
            .contains("EXTERNAL_JUPYTEXT_MARKDOWN")
    );
    let exported = fs::read_to_string(exported_markdown).unwrap();
    assert!(exported.contains("```python") && exported.contains("EXTERNAL_JUPYTEXT_MARKDOWN"));
}
