local M = { version = "0.1.0" }
local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h:h")

local notebook_pattern = "*.ipynb"

local function is_ipynb_buffer(bufnr)
  local name = vim.api.nvim_buf_get_name(bufnr or 0)
  return name:match("%.ipynb$") ~= nil
end

local function is_ipynb_otter_buffer(bufnr)
  local name = vim.api.nvim_buf_get_name(bufnr or 0)
  return name:match("%.ipynb%.otter%.py$") ~= nil
end

local function notebook_python_path()
  return M.options.kernel_python or M.options.python
end

local function notebook_runtime_ready()
  return vim.fn.executable(M.options.python) == 1 and vim.fn.executable(M.options.jupytext) == 1
end

local function notify_missing_runtime()
  vim.notify("run :IpynbInstall, then restart Neovim. see :checkhealth ipynb.", vim.log.levels.WARN)
end

local function highlight_or_nil(name)
  local ok, hl = pcall(vim.api.nvim_get_hl, 0, { name = name, link = false })
  if not ok then
    return {}
  end

  return hl
end

local function apply_notebook_output_highlights()
  local info = highlight_or_nil("DiagnosticInfo")
  local err = highlight_or_nil("DiagnosticError")
  local warn = highlight_or_nil("DiagnosticWarn")
  local success = highlight_or_nil("DiagnosticOk")
  local muted = highlight_or_nil("Comment")
  local normal_float = highlight_or_nil("NormalFloat")
  local float_border = highlight_or_nil("FloatBorder")

  local info_fg = info.fg or 0x61afef
  local err_fg = err.fg or 0xe06c75
  local float_bg = normal_float.bg

  vim.api.nvim_set_hl(0, "IpynbOutputBorder", { fg = muted.fg or float_border.fg or 0x888888, bg = float_bg, bold = true })
  vim.api.nvim_set_hl(0, "IpynbOutputBorderQueued", { fg = warn.fg or 0xe5c07b, bg = float_bg, bold = true })
  vim.api.nvim_set_hl(0, "IpynbOutputBorderRunning", { fg = info_fg, bg = float_bg, bold = true })
  vim.api.nvim_set_hl(0, "IpynbOutputBorderSuccess", { fg = success.fg or 0x98c379, bg = float_bg, bold = true })
  vim.api.nvim_set_hl(0, "IpynbOutputBorderFail", { fg = err_fg, bg = float_bg, bold = true })
  vim.api.nvim_set_hl(0, "IpynbOutputWin", { link = "NormalFloat" })
  vim.api.nvim_set_hl(0, "IpynbOutputWinNC", { link = "NormalFloat" })
  vim.api.nvim_set_hl(0, "IpynbOutputFooter", { fg = info_fg, bg = float_bg, bold = true })
  vim.api.nvim_set_hl(0, "IpynbVirtualText", { fg = info_fg, bg = float_bg })
end

local function configure_notebook_python_lsp(client)
  local python_path = notebook_python_path()

  client.settings = client.settings or {}
  client.settings.python = vim.tbl_deep_extend("force", client.settings.python or {}, {
    pythonPath = python_path,
    analysis = vim.tbl_deep_extend("force", (client.settings.python or {}).analysis or {}, {
      autoSearchPaths = true,
      diagnosticMode = "openFilesOnly",
      useLibraryCodeForTypes = true,
    }),
  })

  client.notify("workspace/didChangeConfiguration", { settings = client.settings })
end

local function ensure_notebook_rendering(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) or not is_ipynb_buffer(bufnr) then
    return
  end

  vim.bo[bufnr].swapfile = false
  vim.bo[bufnr].filetype = "markdown"
  vim.wo.conceallevel = 3
  pcall(vim.treesitter.start, bufnr, "markdown")

  local render_ok, render = pcall(require, "render-markdown")
  if render_ok then
    pcall(vim.api.nvim_buf_call, bufnr, function()
      render.buf_disable()
    end)
  end

  local table_wrap_ok, table_wrap = pcall(require, "markdown-table-wrap")
  if table_wrap_ok then
    pcall(vim.api.nvim_buf_call, bufnr, function()
      table_wrap.disable_auto_preview()
    end)
  end

  require("ipynb.markdown").enable(bufnr)
end

local function available_kernels()
  local ok, kernels = pcall(vim.fn.IpynbAvailableKernels)
  if not ok or type(kernels) ~= "table" then
    return {}
  end

  return kernels
end

local function default_kernel_name()
  local kernels = available_kernels()
  local requested = M.options.kernel
  local path = vim.api.nvim_buf_get_name(0)
  local read_kernel = vim.b.ipynb_read_kernel
  if not requested and read_kernel ~= nil then
    requested = read_kernel or nil
  elseif not requested and vim.fn.filereadable(path) == 1 then
    local ok, notebook = pcall(vim.json.decode, table.concat(vim.fn.readfile(path), "\n"))
    if ok then requested = ((notebook.metadata or {}).kernelspec or {}).name end
  end
  if requested then
    return vim.tbl_contains(kernels, requested) and requested or nil
  end

  if vim.tbl_contains(kernels, "python3") then
    return "python3"
  end

  return kernels[1]
end

local function ensure_notebook_kernel(bufnr)
  if not is_ipynb_buffer(bufnr) then
    return false
  end

  if not notebook_runtime_ready() then
    notify_missing_runtime()
    return false
  end

  if vim.b[bufnr].notebook_kernel_initialized then
    local status_ok, status = pcall(require, "ipynb.molten_status")
    if status_ok and status.initialized() == "Ipynb" then
      return true
    end

    vim.b[bufnr].notebook_kernel_initialized = false
  end

  local kernel = default_kernel_name()
  if not kernel then
    vim.notify(
      "the requested Jupyter kernel is unavailable. see :checkhealth ipynb.",
      vim.log.levels.WARN,
      { title = "notebook kernel" }
    )
    return false
  end

  local ok = pcall(vim.api.nvim_buf_call, bufnr, function()
    vim.cmd("IpynbInit " .. kernel)

    local path = vim.api.nvim_buf_get_name(bufnr)
    if path ~= "" and vim.fn.filereadable(path) == 1 then
      pcall(vim.cmd, "IpynbImportOutput")
    end
  end)

  if ok and #vim.fn.IpynbRunningKernels(true) > 0 then
    vim.b[bufnr].notebook_kernel_initialized = true
    return true
  end

  return false
end

local function reinit_notebook_kernel(bufnr)
  if not is_ipynb_buffer(bufnr) or not notebook_runtime_ready() then return false end
  if #vim.fn.IpynbRunningKernels(true) == 0 then return ensure_notebook_kernel(bufnr) end
  -- the native restart keeps displayed results, including unsaved outputs.
  return pcall(vim.api.nvim_buf_call, bufnr, function() vim.cmd("IpynbRestart") end)
end

local function set_notebook_keymaps(bufnr)
  if vim.b[bufnr].notebook_keymaps_set then
    return
  end

  local runner_ok, runner = pcall(require, "quarto.runner")
  if not runner_ok then
    return
  end

  local function open_output()
    local notebook_buf = vim.api.nvim_get_current_buf()
    vim.cmd("noautocmd IpynbEnterOutput")
    if vim.api.nvim_get_current_buf() ~= notebook_buf then
      for _, key in ipairs({ "<localleader>O", "q", "<Esc>" }) do
        vim.keymap.set("n", key, "<cmd>IpynbHideOutput<cr>", {
          buffer = true,
          silent = true,
          desc = "close notebook output",
        })
      end
    end
  end

  local function open_rich_output()
    if pcall(vim.cmd, "IpynbImagePopup") then
      return
    end

    if pcall(vim.cmd, "IpynbOpenInBrowser") then
      return
    end

    vim.notify("no rich output is available for the active cell.", vim.log.levels.INFO, { title = "notebook output" })
  end

  local function map(mode, lhs, rhs, desc)
    if not M.options.keymaps then return end
    local key = lhs:gsub("<localleader>", vim.g.maplocalleader or "\\")
    if vim.fn.maparg(key, mode) == "" then
      vim.keymap.set(mode, lhs, rhs, { buffer = bufnr, silent = true, desc = desc })
    end
  end

  local function run_with_kernel(fn)
    return function()
      if not ensure_notebook_kernel(bufnr) then
        vim.notify("notebook kernel is unavailable. try <localleader>i.", vim.log.levels.WARN, {
          title = "notebook kernel",
        })
        return
      end

      -- quarto's runner reads a global config; restore it after this notebook call.
      local previous = QuartoConfig.codeRunner
      QuartoConfig.codeRunner = vim.tbl_extend("force", previous, { default_method = function(cell, ignore_cols)
        local first, last = cell.range.from, cell.range.to
        if ignore_cols then
          vim.fn.IpynbEvaluateRange(first[1] + 1, last[1])
        else
          vim.fn.IpynbEvaluateRange(first[1] + 1, last[1], first[2] + 1, last[2] + 1)
        end
      end, ft_runners = {} })
      local ok, err = pcall(fn)
      QuartoConfig.codeRunner = previous
      if not ok then error(err) end
      return true
    end
  end

  local run_cell = run_with_kernel(runner.run_cell)
  local run_all = run_with_kernel(runner.run_all)
  local run_above = run_with_kernel(runner.run_above)
  local function advance()
    if not require("ipynb.cells").current() then
      vim.notify("place the cursor inside a code cell", vim.log.levels.INFO)
      return
    end
    if run_cell() then require("ipynb.cells").advance() end
  end
  local function restart()
    if not reinit_notebook_kernel(bufnr) then
      vim.notify("notebook kernel restart failed", vim.log.levels.WARN)
    end
  end
  local function add_code() require("ipynb.cells").add("code") end
  local function add_markdown() require("ipynb.cells").add("markdown") end
  local function interrupt() vim.cmd("IpynbInterrupt") end
  local function hide() vim.cmd("IpynbHideOutput") end
  local actions = {
    { "run cell", run_cell }, { "run and advance", advance },
    { "run all cells", run_all }, { "run cell and above", run_above },
    { "add code cell below", add_code }, { "add markdown cell below", add_markdown },
    { "open full output", open_output }, { "close full output", hide },
    { "open rich output", open_rich_output }, { "interrupt execution", interrupt },
    { "restart kernel (clears variables)", restart },
  }
  local function menu()
    vim.ui.select(actions, { prompt = "notebook", format_item = function(item) return item[1] end }, function(item)
      if item then item[2]() end
    end)
  end
  vim.api.nvim_buf_create_user_command(bufnr, "Ipynb", menu, { desc = "notebook actions" })
  map("n", "<localleader>n", menu, "notebook actions")
  map("n", "<localleader>r", run_cell, "run cell")
  map("v", "<localleader>r", run_with_kernel(runner.run_range), "run selection")
  map("n", "<C-CR>", run_cell, "run cell")
  map("n", "<S-CR>", advance, "run and advance")
  map("n", "<localleader>R", run_all, "run all cells")
  map("n", "<localleader>a", run_above, "run cell and above")
  map("n", "<localleader>b", add_code, "add code cell below")
  map("n", "<localleader>m", add_markdown, "add markdown cell below")
  map("n", "<localleader>s", interrupt, "interrupt execution")
  map("n", "<localleader>o", open_output, "open full output")
  map("n", "<localleader>O", hide, "close full output")
  map("n", "<localleader>x", open_rich_output, "open rich output")
  map("n", "<localleader>i", restart, "restart kernel")

  vim.b[bufnr].notebook_keymaps_set = true
end

local function activate_notebook_buffer(bufnr)
  if not is_ipynb_buffer(bufnr) then
    return
  end

  vim.b[bufnr].notebook_markdown = true
  vim.b[bufnr].autoformat = false
  if vim.b[bufnr].completion == nil then vim.b[bufnr].completion = true end
  vim.bo[bufnr].swapfile = false
  vim.bo[bufnr].buftype = "acwrite"
  vim.wo.spell = false

  if not vim.api.nvim_buf_is_valid(bufnr) or not is_ipynb_buffer(bufnr) then
    return
  end

  vim.api.nvim_buf_call(bufnr, function()
    require("otter").activate({ "python" }, true, true)
  end)

  vim.schedule(function()
    ensure_notebook_rendering(bufnr)
  end)
  set_notebook_keymaps(bufnr)
  if M.options.auto_init then ensure_notebook_kernel(bufnr) end
end

function M.init(opts)
  local venv = vim.fn.stdpath("data") .. "/ipynb.nvim/venv"
  M.options = vim.tbl_deep_extend("force", {
    python = vim.g.python3_host_prog or venv .. "/bin/python",
    images = true,
    auto_init = true,
    numpy_legacy_repr = true,
    keymaps = true,
    output = { preview_lines = 8 },
  }, opts or {})
  M.options.jupytext = M.options.jupytext or vim.fn.fnamemodify(M.options.python, ":h") .. "/jupytext"
  vim.g.python3_host_prog = M.options.python
  vim.g.ipynb_numpy_legacy_repr = M.options.numpy_legacy_repr
  vim.g.ipynb_auto_init_behavior = "raise"
  vim.g.ipynb_auto_open_output = false
  vim.g.ipynb_enter_output_behavior = "open_and_enter"
  local images = M.options.images and #vim.api.nvim_list_uis() > 0
  vim.g.ipynb_image_provider = images and "image.nvim" or "none"
  vim.g.ipynb_image_location = images and "both" or "float"
  vim.g.ipynb_output_virt_lines = true
  vim.g.ipynb_output_show_more = true
  vim.g.ipynb_use_border_highlights = true
  vim.g.ipynb_output_win_border = { "╭", "─", "╮", "│", "╯", "─", "╰", "│" }
  vim.g.ipynb_output_win_hide_on_leave = false
  vim.g.ipynb_output_win_max_height = M.options.output.height
  vim.g.ipynb_output_win_max_width = M.options.output.width
  vim.g.ipynb_wrap_output = true
  vim.g.ipynb_virt_text_output = true
  vim.g.ipynb_virt_text_max_lines = M.options.output.preview_lines
  vim.g.ipynb_virt_lines_off_by_1 = true
end

function M.install()
  if not M.options then M.init() end
  local venv = vim.fn.stdpath("data") .. "/ipynb.nvim/venv"
  if M.options.python == venv .. "/bin/python" then
    local result = vim.system({ "python3", root .. "/scripts/setup.py", "--venv", venv }, { text = true }):wait()
    if result.code ~= 0 then error(result.stderr .. result.stdout) end
  else
    local result = vim.system({ M.options.python, "-c",
      "import pynvim, jupyter_client, ipykernel, jupytext, nbformat, numpy, matplotlib, PIL",
    }, { text = true }):wait()
    if result.code ~= 0 then
      error("custom python runtime needs requirements.txt installed first: " .. (result.stderr or ""))
    end
  end
  vim.g.python3_host_prog = M.options.python
  local treesitter = require("nvim-treesitter")
  if treesitter.install then
    treesitter.install({ "markdown", "markdown_inline", "python" }):wait(300000)
  else
    vim.cmd("TSInstallSync markdown markdown_inline python")
  end
  if M.options.backend == "python" then
    vim.cmd("runtime! plugin/rplugin.vim")
    vim.cmd("UpdateRemotePlugins")
  else
    if vim.fn.executable("cargo") ~= 1 then error("install Rust with rustup, then run :IpynbInstall again") end
    local result = vim.system({ "cargo", "build", "--release", "--locked" },
      { cwd = root .. "/native", text = true }):wait()
    if result.code ~= 0 then error("Rust engine build failed: " .. (result.stderr or result.stdout)) end
  end
  vim.notify("notebook runtime installed. restart neovim before opening notebooks.")
end

function M.setup(opts)
  M.init(opts)
  vim.api.nvim_create_user_command("IpynbInstall", M.install, { desc = "Install notebook python tools" })
  require("jupytext").setup({
    jupytext = M.options.jupytext,
    format = "md:markdown",
    filetype = "markdown",
    new_template = root .. "/data/template.ipynb",
    autosync = false,
    sync_patterns = {},
    handle_url_schemes = false,
    update = true,
    async_write = false,
  })
  require("ipynb.native").setup(M.options)
  require("ipynb.notebook_io").setup(require("jupytext"))
  local group = vim.api.nvim_create_augroup("ipynb", { clear = true })
  local function autocmd(events, callback)
    vim.api.nvim_create_autocmd(events, { group = group, pattern = "*.ipynb", callback = callback })
  end
  autocmd({ "BufReadPre", "BufNewFile" }, function(ev) vim.bo[ev.buf].swapfile = false end)
  autocmd({ "BufReadPost", "BufEnter" }, function(ev) activate_notebook_buffer(ev.buf) end)
  autocmd({ "BufWinEnter", "BufWritePost", "InsertLeave", "TextChanged" }, function(ev)
    require("ipynb.markdown").enable(ev.buf)
    vim.wo.spell = false
  end)
  autocmd("InsertEnter", function(ev) require("ipynb.markdown").disable(ev.buf) end)
  autocmd("BufWritePost", function(ev)
    if vim.b[ev.buf].ipynb_native_write then
      vim.b[ev.buf].ipynb_native_write = nil
      return
    end
    if vim.fn.filereadable(vim.api.nvim_buf_get_name(ev.buf)) == 0 then return end
    -- jupytext suppresses autocmd errors, so report export failures explicitly.
    local ok, err = pcall(vim.api.nvim_buf_call, ev.buf, function()
      if #vim.fn.IpynbRunningKernels(true) > 0 then vim.cmd("IpynbExportOutput!") end
    end)
    if not ok then
      vim.schedule(function() vim.notify("could not save notebook outputs: " .. tostring(err), vim.log.levels.ERROR) end)
    end
    vim.schedule(function()
      if not vim.api.nvim_buf_is_valid(ev.buf) then return end
      local stat = vim.uv.fs_stat(vim.api.nvim_buf_get_name(ev.buf))
      if stat then vim.b[ev.buf].mtime = stat.mtime end
    end)
  end)
  apply_notebook_output_highlights()
  vim.api.nvim_create_autocmd("ColorScheme", { group = group, callback = apply_notebook_output_highlights })
  vim.api.nvim_create_autocmd("User", { group = group, pattern = "VeryLazy", callback = function()
    if is_ipynb_buffer(0) then vim.wo.spell = false end
  end })
  vim.api.nvim_create_autocmd("LspAttach", { group = group, callback = function(ev)
    local client = vim.lsp.get_client_by_id(ev.data.client_id)
    if not client then return end
    if client.name == "marksman" and is_ipynb_buffer(ev.buf) then
      vim.schedule(function()
        if vim.api.nvim_buf_is_valid(ev.buf) then vim.lsp.buf_detach_client(ev.buf, client.id) end
      end)
    elseif M.options.kernel_python and client.name == "pyright" and is_ipynb_otter_buffer(ev.buf) then
      configure_notebook_python_lsp(client)
    end
  end })
end

return M
