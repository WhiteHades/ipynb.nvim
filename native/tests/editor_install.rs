//! Check the installer's core editor flow without changing host tools.
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn editor_install_checks_prerequisites_runtime_and_new_parsers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let base = root.join(".tmp/editor-install");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::tempdir_in(base).unwrap();
    let script = fixture.path().join("check.lua");
    fs::write(&script, r#"
local ok, err = xpcall(function()
  vim.opt.rtp:prepend(vim.env.IPYNB_INSTALL_ROOT)
  local ipynb = require('ipynb')
  local commands, ready, parser_failure = {}, true, false
  vim.notify = function() end
  vim.system = function(command)
    commands[#commands + 1] = vim.deepcopy(command)
    return { wait = function() return { code = ready and 0 or 1, stdout = 'missing tool', stderr = '' } end }
  end
  local site = vim.fn.stdpath('data') .. '/site'
  vim.opt.rtp:remove(site)
  package.loaded['nvim-treesitter.config'] = { get_install_dir = function() return site end }
  assert(#vim.api.nvim_get_runtime_file('parser/python.so', false) == 0)
  package.loaded['nvim-treesitter'] = { install = function()
    return { wait = function()
      vim.fn.mkdir(site .. '/parser', 'p')
      vim.fn.writefile({}, site .. '/parser/python.so')
    end }
  end }
  vim.treesitter.language.add = function()
    if parser_failure then return nil, 'missing parser' end
    assert(#vim.api.nvim_get_runtime_file('parser/python.so', false) == 1, 'new parser directory is invisible')
    return true
  end
  ipynb.init({ images = false })
  ipynb.install()
  assert(commands[1][3] == '--check' and commands[2][1] == 'uv' and commands[2][5] == '3.15.0')
  assert(commands[3][1] == 'cargo' and #commands == 3)

  commands, ready = {}, false
  local installed, failure = pcall(ipynb.install)
  assert(not installed and failure:find('Run in a terminal:', 1, true) and #commands == 1)

  commands, ready = {}, true
  ipynb.init({ python = '/custom/python', images = false })
  ipynb.install()
  assert(commands[2][1] == '/custom/python' and commands[2][2] == '-c' and #commands == 3)
  commands, parser_failure = {}, true
  installed, failure = pcall(ipynb.install)
  assert(not installed and failure:find('missing parser', 1, true) and #commands == 2)
end, debug.traceback)
if not ok then io.stderr:write(err .. '\n'); vim.cmd('cquit 1') end
vim.cmd('qa!')
"#).unwrap();
    let log = fixture.path().join("nvim.log");
    let output = fs::File::create(&log).unwrap();
    let mut child =
        Command::new(std::env::var_os("IPYNB_TEST_NVIM").unwrap_or_else(|| "nvim".into()))
            .args(["--headless", "-u", "NONE", "-l"])
            .arg(script)
            .env("IPYNB_INSTALL_ROOT", root)
            .env("XDG_DATA_HOME", fixture.path().join("data"))
            .env("XDG_STATE_HOME", fixture.path().join("state"))
            .env("XDG_CACHE_HOME", fixture.path().join("cache"))
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(15) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "editor installer timed out: {}",
                fs::read_to_string(&log).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{}", fs::read_to_string(log).unwrap());
}
