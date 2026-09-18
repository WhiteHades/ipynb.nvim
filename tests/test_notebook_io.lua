vim.cmd([[
  function! IpynbNotebookRead(path, template) abort
    let g:rpc_read_calls = get(g:, 'rpc_read_calls', 0) + 1
    if get(g:, 'rpc_read_fail', 0)
      throw 'read failed'
    endif
    return {'text': "loaded\nnotebook", 'metadata': {'from': 'rpc'}}
  endfunction

  function! IpynbNotebookWrite(path, text, expected_mtime) abort
    let g:rpc_write_calls = get(g:, 'rpc_write_calls', 0) + 1
    if get(g:, 'rpc_write_fail', 0)
      throw 'write failed'
    endif
    return {'mtime': {'sec': 10, 'nsec': 20}}
  endfunction
]])

vim.g.rpc_read_calls = 0
vim.g.rpc_write_calls = 0
vim.g.rpc_write_fail = 0

local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
vim.opt.rtp:prepend(root)

local settings = {
  format = "md:markdown",
  autosync = false,
  async_write = false,
  update = true,
  filetype = "markdown",
  new_template = "template.ipynb",
}
local original_reads, original_writes = 0, 0
local jupytext = {
  get_option = function(name)
    return settings[name]
  end,
  open_notebook = function()
    original_reads = original_reads + 1
    return "original-read"
  end,
  write_notebook = function()
    original_writes = original_writes + 1
    return "original-write"
  end,
}
local io = require("ipynb.notebook_io")
io.setup(jupytext)

local buffer = vim.api.nvim_get_current_buf()
assert(jupytext.open_notebook("local.ipynb", buffer).from == "rpc")
assert(vim.api.nvim_buf_get_lines(buffer, 0, -1, false)[1] == "loaded")
assert(vim.bo[buffer].filetype == "markdown")
assert(not vim.bo[buffer].modified)
assert(vim.g.rpc_read_calls == 1)

local path = vim.fn.tempname() .. ".ipynb"
vim.fn.writefile({ "existing" }, path)
vim.api.nvim_buf_set_name(buffer, path)
vim.b[buffer].mtime = { sec = 1, nsec = 2 }
vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { "changed" })
vim.api.nvim_set_option_value("modified", true, { buf = buffer })
assert(vim.bo[buffer].modified)
local result = jupytext.write_notebook(path, {}, buffer)
assert(result.mtime.sec == 10 and result.mtime.nsec == 20)
assert(not vim.bo[buffer].modified)
assert(vim.b[buffer].mtime.sec == 10 and vim.b[buffer].mtime.nsec == 20)
assert(vim.g.rpc_write_calls == 1)

vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { "failed" })
vim.api.nvim_set_option_value("modified", true, { buf = buffer })
assert(vim.bo[buffer].modified)
vim.g.rpc_write_fail = 1
local ok = pcall(jupytext.write_notebook, path, {}, buffer)
assert(not ok and vim.bo[buffer].modified)
assert(original_writes == 0 and vim.g.rpc_write_calls == 2)
vim.g.rpc_write_fail = 0

settings.format = "py:percent"
assert(jupytext.open_notebook("unsupported.ipynb", buffer) == "original-read")
assert(jupytext.write_notebook("unsupported.ipynb", {}, buffer) == "original-write")
assert(original_reads == 1 and original_writes == 1)
assert(vim.g.rpc_read_calls == 1 and vim.g.rpc_write_calls == 2)

settings.format = "md:markdown"
vim.cmd("delfunction IpynbNotebookRead")
assert(jupytext.open_notebook("old-manifest.ipynb", buffer) == "original-read")
assert(jupytext.write_notebook("old-manifest.ipynb", {}, buffer) == "original-write")
assert(original_reads == 2 and original_writes == 2)

vim.fn.delete(path)
print("pass: notebook IO success, failure, and fallback behavior")
vim.cmd("qa!")
