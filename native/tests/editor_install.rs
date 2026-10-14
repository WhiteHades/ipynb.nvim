//! Real headless Neovim checks with process, terminal, and UI boundaries stubbed.
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn editor_check(body: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let temporary = root.join(".tmp/editor-install");
    fs::create_dir_all(&temporary).unwrap();
    let fixture = tempfile::tempdir_in(&temporary).unwrap();
    let script = fixture.path().join("fixture.lua");
    let lua = format!(
        r#"
local ok, err = xpcall(function()
  local root = vim.env.IPYNB_INSTALL_ROOT
  vim.opt.rtp:prepend(root)
  local calls, notices = {{}}, {{}}
  local ready, import_failure = true, false
  vim.notify = function(message) notices[#notices + 1] = message end
  vim.system = function(command, options)
    calls[#calls + 1] = vim.deepcopy(command)
    assert(options.text == true)
    return {{ wait = function()
      return {{ code = (command[1] == 'bash' and not ready or command[2] == '-c' and import_failure) and 1 or 0,
        stdout = ready and '' or 'missing tool', stderr = '' }}
    end }}
  end
  local function reset()
    calls, notices = {{}}, {{}}
  end
  {body}
end, debug.traceback)
if not ok then
  io.stderr:write(err .. '\n')
  vim.cmd('cquit 1')
else
  io.stdout:write('editor install check passed\n')
  vim.cmd('qa!')
end
"#
    );
    fs::write(&script, lua).unwrap();
    let nvim = std::env::var_os("IPYNB_TEST_NVIM").unwrap_or_else(|| "nvim".into());
    let stdout = fs::File::create(fixture.path().join("stdout")).unwrap();
    let stderr = fs::File::create(fixture.path().join("stderr")).unwrap();
    let mut child = Command::new(nvim)
        .args(["-u", "NONE", "--headless", "-l"])
        .arg(&script)
        .env("IPYNB_INSTALL_ROOT", root)
        .env("XDG_DATA_HOME", fixture.path().join("data"))
        .env("XDG_STATE_HOME", fixture.path().join("state"))
        .env("XDG_CACHE_HOME", fixture.path().join("cache"))
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > Duration::from_secs(15) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Neovim installer check timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = fs::read_to_string(fixture.path().join("stdout")).unwrap();
    let stderr = fs::read_to_string(fixture.path().join("stderr")).unwrap();
    assert!(status.success(), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("editor install check passed"), "{stdout}");
}

#[test]
fn ready_runtime_preserves_custom_python_and_rejects_missing_parsers() {
    editor_check(
        r#"
  local ipynb = require('ipynb')
  local parser_failure = false
  local parser_directory = vim.fn.stdpath('data') .. '/site/parser'
  vim.opt.rtp:prepend(vim.fn.stdpath('data') .. '/site')
  assert(#vim.api.nvim_get_runtime_file('parser/python.so', false) == 0)
  package.loaded['nvim-treesitter'] = { install = function(languages)
    assert(vim.deep_equal(languages, { 'markdown', 'markdown_inline', 'python' }))
    return { wait = function(_, timeout)
      assert(timeout == 300000)
      vim.fn.mkdir(parser_directory, 'p')
      vim.fn.writefile({}, parser_directory .. '/python.so')
    end }
  end }
  vim.treesitter.language.add = function(language)
    if parser_failure and language == 'python' then return nil, 'missing parser' end
    assert(#vim.api.nvim_get_runtime_file('parser/python.so', false) == 1, 'new parser directory is not visible')
    return true
  end
  ipynb.init({ images = false })
  ipynb.install()
  assert(vim.deep_equal(calls[1], { 'bash', root .. '/scripts/prerequisites.sh', '--check', '--no-images' }))
  assert(vim.deep_equal(calls[2], { 'uv', 'run', '--no-project', '--python', '3.15.0', '--',
    root .. '/scripts/setup.py', '--venv', vim.fn.stdpath('data') .. '/ipynb.nvim/venv' }))
  assert(vim.deep_equal(calls[3], { 'cargo', 'build', '--release', '--locked' }))
  assert(#notices == 1)

  reset()
  ipynb.init({ python = '/custom/runtime/python', images = false })
  ipynb.install()
  assert(#calls == 3)
  assert(calls[2][1] == '/custom/runtime/python' and calls[2][2] == '-c')
  assert(calls[2][3]:find('import pynvim', 1, true))
  for _, command in ipairs(calls) do
    assert(command[1] ~= 'uv')
    for _, argument in ipairs(command) do
      assert(not argument:find('pip', 1, true) and not argument:find('setup.py', 1, true))
    end
  end

  reset()
  import_failure = true
  local valid, validation_error = pcall(ipynb.install)
  assert(not valid and validation_error:find('custom python runtime needs requirements.txt installed first', 1, true))
  assert(#calls == 2 and #notices == 0, 'invalid custom runtime must stop without mutation')

  reset()
  import_failure, parser_failure = false, true
  local succeeded, failure = pcall(ipynb.install)
  assert(not succeeded and failure:find('Notebook parser installation failed for python', 1, true))
  assert(failure:find('missing parser', 1, true))
  assert(#calls == 2 and #notices == 0, 'parser failure must block Cargo and success notice')
"#,
    );
}

#[test]
fn headless_missing_prerequisites_stop_before_runtime_and_preserve_flags() {
    editor_check(
        r#"
  local ipynb = require('ipynb')
  assert(#vim.api.nvim_list_uis() == 0)
  ready = false
  ipynb.init({ images = false, backend = 'python' })
  vim.fn.jobstart = function() error('headless installation must not start a terminal') end
  local succeeded, failure = pcall(ipynb.install)
  assert(not succeeded and failure:find('Run in a terminal:', 1, true))
  assert(failure:find('--install', 1, true) and failure:find('--no-images', 1, true))
  assert(failure:find('--python-backend', 1, true))
  assert(#calls == 1 and #notices == 0, 'headless refusal must precede runtime changes')
  assert(vim.deep_equal(calls[1], { 'bash', root .. '/scripts/prerequisites.sh',
    '--check', '--no-images', '--python-backend' }))
"#,
    );
}

#[test]
fn terminal_completion_blocks_duplicates_and_suspends_lazy_builder() {
    editor_check(
        r#"
  local gate = require('ipynb.install')
  local exits, installs = {}, 0
  local options = { images = false, backend = 'python' }
  vim.api.nvim_list_uis = function() return { {} } end
  vim.fn.jobstart = function(command, job_options)
    assert(job_options.term == true)
    assert(vim.deep_equal(command, { 'bash', root .. '/scripts/prerequisites.sh',
      '--install', '--no-images', '--python-backend' }))
    exits[#exits + 1] = job_options.on_exit
    return #exits
  end
  local function install() installs = installs + 1; return 'installed' end
  ready = false
  gate.run(root, options, install)
  assert(installs == 0 and #exits == 1)
  local accepted, duplicate = pcall(gate.run, root, options, install)
  assert(not accepted and duplicate:find('already running', 1, true))
  ready = true
  exits[1](1, 0)
  assert(installs == 0, 'on_exit must schedule continuation')
  assert(vim.wait(2000, function() return installs == 1 end), 'success callback did not finish')
  assert(calls[#calls][3] == '--check', 'terminal success must recheck prerequisites')

  ready = false
  gate.run(root, options, install)
  exits[2](2, 130)
  assert(vim.wait(2000, function() return #notices == 1 end), 'cancellation callback did not finish')
  assert(installs == 1 and notices[1]:find('cancelled', 1, true))

  -- Model lazy.async's coroutine contract without requiring installed plugins.
  local task, worker, returned, resumed, worker_error = {}, nil, false, false, nil
  package.loaded['lazy.async'] = { running = function()
    return coroutine.running() == worker and task or nil
  end }
  function task:suspend() coroutine.yield() end
  function task:resume()
    resumed = true
    local ok, err = coroutine.resume(worker)
    if not ok then worker_error = err end
  end
  worker = coroutine.create(function()
    assert(gate.run(root, options, install) == 'installed')
    returned = true
  end)
  assert(coroutine.resume(worker))
  assert(coroutine.status(worker) == 'suspended' and not returned and installs == 1)
  ready = true
  exits[3](3, 0)
  assert(not returned and not resumed, 'lazy builder returned before scheduled completion')
  assert(vim.wait(2000, function() return resumed end), 'lazy resume callback did not finish')
  assert(not worker_error, worker_error)
  assert(returned and coroutine.status(worker) == 'dead' and installs == 2)
"#,
    );
}
