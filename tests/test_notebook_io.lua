vim.cmd([[
  function! IpynbNotebookRead(path, template) abort
    let g:rpc_read_calls = get(g:, 'rpc_read_calls', 0) + 1
    if get(g:, 'rpc_read_fail', 0)
      throw 'read failed'
    endif
    return {'text': "loaded\nnotebook", 'metadata': {'from': 'rpc', 'kernelspec': {'name': 'python3'}}}
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
  jupytext = "jupytext-fixture",
}
local original_reads, original_writes = 0, 0
local jupytext = {
  get_option = function(name)
    return settings[name]
  end,
  read_file = function()
    original_reads = original_reads + 1
    return { "{}" }
  end,
  get_metadata = function() return { from = "fallback" } end,
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
assert(vim.b[buffer].ipynb_read_kernel == "python3")
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
local system = vim.system
local command_failed = false
vim.system = function(command, opts)
  assert(command[1] == "jupytext-fixture" and command[5] == "py:percent")
  assert(vim.deep_equal(opts.stdin, { "{}" }))
  return { wait = function() return { code = command_failed and 1 or 0, stdout = "print(1)\n", stderr = "fixture" } end }
end
assert(jupytext.open_notebook("unsupported.ipynb", buffer).from == "fallback")
assert(vim.api.nvim_buf_get_lines(buffer, 0, -1, false)[1] == "print(1)")
assert(vim.b[buffer].ipynb_read_kernel == nil)
assert(jupytext.write_notebook("unsupported.ipynb", {}, buffer) == "original-write")
assert(original_reads == 1 and original_writes == 1)
assert(vim.g.rpc_read_calls == 1 and vim.g.rpc_write_calls == 2)
command_failed = true
assert(not pcall(jupytext.open_notebook, "unsupported.ipynb", buffer), "fallback conversion failure must propagate")
assert(vim.b[buffer].ipynb_read_incomplete and not pcall(jupytext.write_notebook, path, {}, buffer))
assert(original_writes == 1, "incomplete fallback reads must not reach the writer")
assert(vim.api.nvim_buf_get_lines(buffer, 0, -1, false)[1] == "print(1)", "failed conversion must preserve buffer contents")
command_failed = false
settings.format = function(_, metadata) assert(metadata.from == "fallback"); return "py:percent" end
local filetype = settings.filetype
settings.filetype = function(_, format) assert(format == "py:percent"); return "markdown" end
assert(jupytext.open_notebook("unsupported.ipynb", buffer).from == "fallback")
assert(not vim.b[buffer].ipynb_read_incomplete, "a successful fallback read must restore saving")
settings.filetype, vim.system = filetype, system

settings.format = "md:markdown"
vim.cmd("delfunction IpynbNotebookRead")
assert(not pcall(jupytext.open_notebook, "missing-engine.ipynb", buffer))
assert(original_reads == 3, "a missing engine must not silently use another converter")

-- Remote functions exist only as FuncUndefined handlers until their first call.
vim.api.nvim_create_autocmd("FuncUndefined", {
  pattern = "IpynbNotebookRead",
  once = true,
  callback = function()
    vim.cmd([[
      function! IpynbNotebookRead(path, template) abort
        return {'text': 'lazy loaded', 'metadata': {'lazy': 1}}
      endfunction
    ]])
  end,
})
assert(vim.fn.exists("*IpynbNotebookRead") == 0)
assert(jupytext.open_notebook("lazy.ipynb", buffer).lazy == 1)
assert(vim.b[buffer].ipynb_read_kernel == false)
assert(original_reads == 3)

vim.cmd([[
  function! IpynbNotebookRead(path, template) abort
    let g:rpc_read_buffer = bufnr()
    let g:rpc_read_readonly = &l:readonly
    return {'text': 'recovered', 'metadata': {'from': 'recovered'}, 'recovery_pending': v:true}
  endfunction
]])
local hidden = vim.api.nvim_create_buf(false, true)
vim.bo[hidden].readonly = true
assert(jupytext.open_notebook("hidden.ipynb", hidden).from == "recovered")
assert(vim.g.rpc_read_buffer == hidden and vim.g.rpc_read_readonly == 1,
  "opening a noncurrent notebook must use the target buffer's readonly setting")
assert(vim.api.nvim_get_current_buf() == buffer)
local recovery_path = vim.fn.tempname() .. ".ipynb"
local recovery_lines = { "---", "jupyter:", "---", "", "```python", "print(42)", "```" }
vim.fn.writefile(recovery_lines, recovery_path)
settings.format, settings.autosync = "py:percent", true
jupytext.read_file = function(file) return vim.fn.readfile(file) end
vim.system = function(command, opts)
  assert(command[2] == "--from" and command[3] == "md:markdown" and command[5] == "py:percent",
    "readonly recovered Markdown must not run sync or be parsed as JSON")
  assert(vim.deep_equal(opts.stdin, recovery_lines))
  return { wait = function() return { code = 0, stdout = "print(42)\n" } end }
end
assert(jupytext.open_notebook(recovery_path, hidden).from == "recovered")
assert(vim.g.rpc_read_buffer == hidden and vim.g.rpc_read_readonly == 1)
assert(vim.deep_equal(vim.fn.readfile(recovery_path), recovery_lines))
vim.system = system
vim.fn.delete(recovery_path)
vim.api.nvim_buf_delete(hidden, { force = true })
vim.fn.delete(path)
print("pass: notebook IO success, failure, and fallback behavior")
vim.cmd("qa!")
