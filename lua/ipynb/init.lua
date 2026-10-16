local M = { version = "0.1.3" }
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

local function notebook_python_path(bufnr)
  return require("ipynb.project").python(bufnr or 0, M.options)
end

local function notebook_runtime_ready()
  return vim.fn.executable(M.options.python) == 1 and vim.fn.executable(M.options.jupytext) == 1
end

local function notify_missing_runtime()
  vim.notify("run :IpynbInstall, then restart Neovim. see :checkhealth ipynb.", vim.log.levels.WARN)
end

local function apply_notebook_output_highlights()
  require("ipynb.highlights").setup()
end

local function configure_notebook_python_lsp(client, bufnr)
  local python_path = notebook_python_path(bufnr)
  if not python_path then return end
  local project_root = require("ipynb.project").resolve(bufnr, M.options).root
  if project_root and client.config.root_dir ~= project_root then
    vim.schedule(function()
      if not vim.api.nvim_buf_is_valid(bufnr) or not client.attached_buffers[bufnr] then return end
      local config = vim.deepcopy(client.config)
      config.root_dir = project_root
      config.settings = vim.tbl_deep_extend("force", config.settings or {}, { python = { pythonPath = python_path } })
      vim.lsp.buf_detach_client(bufnr, client.id)
      if next(client.attached_buffers) == nil then client.stop() end
      vim.lsp.start(config, { bufnr = bufnr })
    end)
    return
  end
  if client.settings and client.settings.python and client.settings.python.pythonPath == python_path then return end

  client.settings = client.settings or {}
  client.settings.python = vim.tbl_deep_extend("force", client.settings.python or {}, {
    pythonPath = python_path,
  })

  client:notify("workspace/didChangeConfiguration", { settings = client.settings })
end

function M.refresh_python_lsp(bufnr)
  local path = vim.api.nvim_buf_get_name(bufnr) .. ".otter.py"
  local otter = vim.fn.bufnr(path)
  if otter < 0 then return end
  for _, client in ipairs(vim.lsp.get_clients({ bufnr = otter })) do
    if client.name == "pyright" or client.name == "basedpyright" then
      configure_notebook_python_lsp(client, otter)
    end
  end
end

local function ensure_notebook_rendering(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) or not is_ipynb_buffer(bufnr) then
    return
  end

  vim.bo[bufnr].swapfile = false
  if vim.bo[bufnr].filetype ~= "markdown" then vim.bo[bufnr].filetype = "markdown" end
  pcall(require("ipynb.markdown").start_highlight, bufnr)

  local render_ok, render = pcall(require, "render-markdown")
  if render_ok then
    pcall(vim.api.nvim_buf_call, bufnr, function()
      if require("ipynb.markdown_integration").active() then render.buf_enable() else render.buf_disable() end
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

local function default_kernel_name(bufnr)
  local selection = vim.b[bufnr].ipynb_kernel_selection or require("ipynb.project").resolve(bufnr, M.options)
  if selection.error then error(selection.error) end
  if selection.python then return selection end
  if selection.kernel and (selection.kernel:match("^https?://") or selection.kernel:match("%.json$")) then return selection end
  local kernels = available_kernels()
  local requested = selection.kernel
  if requested then
    return vim.tbl_contains(kernels, requested) and selection or nil
  end

  if vim.tbl_contains(kernels, "python3") then
    selection.kernel = "python3"
    return selection
  end

  selection.kernel = kernels[1]
  return selection.kernel and selection or nil
end

local function ensure_notebook_kernel(bufnr)
  if not is_ipynb_buffer(bufnr) then
    return false
  end

  if not notebook_runtime_ready() then
    notify_missing_runtime()
    return false
  end

  local native = require("ipynb.native")
  local running = native.running(bufnr)
  if #running > 0 then return true end
  vim.b[bufnr].notebook_kernel_initialized = false
  local selected, selection = pcall(default_kernel_name, bufnr)
  if not selected then
    vim.notify(tostring(selection), vim.log.levels.ERROR, { title = "notebook kernel" })
    return false
  end
  if not selection then
    vim.notify(
      "the requested Jupyter kernel is unavailable. see :checkhealth ipynb.",
      vim.log.levels.WARN,
      { title = "notebook kernel" }
    )
    return false
  end

  local ok, err = pcall(native.initialize, bufnr, selection)
  if not ok then vim.notify(tostring(err), vim.log.levels.ERROR, { title = "notebook kernel" }); return false end

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

  local function open_output()
    vim.cmd("noautocmd IpynbEnterOutput")
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
    return function(prepared)
      if vim.b[bufnr].ipynb_setup_pending then return end
      local native = require("ipynb.native")
      local selection = vim.b[bufnr].ipynb_kernel_selection or require("ipynb.project").resolve(bufnr, M.options)
      if not prepared and selection and selection.kernel == "project" and #native.running(bufnr) == 0 then
        local tick, cursor = vim.api.nvim_buf_get_changedtick(bufnr), vim.api.nvim_win_get_cursor(0)
        vim.b[bufnr].ipynb_setup_pending = true
        selection.bufnr = bufnr
        require("ipynb.project").prepare(selection, M.options, function(err)
          if not vim.api.nvim_buf_is_valid(bufnr) then return end
          vim.b[bufnr].ipynb_setup_pending = nil
          if err then vim.notify(err, vim.log.levels.ERROR); return end
          if vim.api.nvim_buf_get_changedtick(bufnr) ~= tick then
            vim.notify("Notebook Python is ready. Run the cell again because the notebook changed during setup.")
            return
          end
          vim.api.nvim_buf_call(bufnr, function()
            vim.api.nvim_win_set_cursor(0, cursor)
            M.refresh_python_lsp(bufnr)
            run_with_kernel(fn)(true)
          end)
        end)
        return
      end
      if not ensure_notebook_kernel(bufnr) then
        return
      end

      fn()
      return true
    end
  end

  local function run_cells(scope)
    local row = vim.api.nvim_win_get_cursor(0)[1] - 1
    local spans = require("ipynb.cells").executable(bufnr, true)
    local found = false
    for _, cell in ipairs(spans) do
      local first, last = cell.begin[1] - 1, cell["end"][1] + 1
      if scope == "all" or (scope == "above" and first <= row) or (first <= row and row <= last) then
        vim.fn.IpynbEvaluateRange(cell.begin[1] + 1, cell["end"][1] + 1)
        found = true
      end
    end
    if not found then vim.notify("Place the cursor inside a code cell.", vim.log.levels.INFO) end
  end
  local run_cell = run_with_kernel(function() run_cells("cell") end)
  local run_all = run_with_kernel(function() run_cells("all") end)
  local run_above = run_with_kernel(function() run_cells("above") end)
  local run_selection = run_with_kernel(function() vim.cmd("IpynbEvaluateVisual") end)
  local run_and_advance = run_with_kernel(function()
    run_cells("cell")
    require("ipynb.cells").advance()
  end)
  local function advance()
    if not require("ipynb.cells").current() then
      vim.notify("place the cursor inside a code cell", vim.log.levels.INFO)
      return
    end
    run_and_advance()
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
    { "Jupytext formats and sync", function() require("ipynb.jupytext").run("menu") end },
  }
  local function menu()
    local path, tick = vim.api.nvim_buf_get_name(bufnr), vim.api.nvim_buf_get_changedtick(bufnr)
    vim.ui.select(actions, { prompt = "notebook", format_item = function(item) return item[1] end }, function(item)
      if not item then return end
      if not vim.api.nvim_buf_is_valid(bufnr) or vim.api.nvim_get_current_buf() ~= bufnr
        or vim.api.nvim_buf_get_name(bufnr) ~= path or vim.api.nvim_buf_get_changedtick(bufnr) ~= tick then
        vim.notify("Notebook changed or focus moved. Menu action cancelled; open the menu again.", vim.log.levels.WARN)
        return
      end
      item[2]()
    end)
  end
  vim.api.nvim_buf_create_user_command(bufnr, "Ipynb", menu, { desc = "notebook actions" })
  map("n", "<localleader>n", menu, "notebook actions")
  map("n", "<localleader>r", run_cell, "run cell")
  map("v", "<localleader>r", function()
    vim.cmd.normal({ vim.api.nvim_replace_termcodes("<Esc>", true, false, true), bang = true })
    run_selection()
  end, "run selection")
  map("n", "<C-CR>", run_cell, "run cell")
  map("n", "<S-CR>", advance, "run and advance")
  map("n", "<localleader>R", run_all, "run all cells")
  map("n", "<localleader>a", run_above, "run cell and above")
  map("n", "<localleader>b", add_code, "add code cell below")
  map("n", "<localleader>m", add_markdown, "add markdown cell below")
  map("n", "<localleader>s", interrupt, "interrupt execution")
  map("n", "<Plug>(ipynb-toggle-output)", open_output, "toggle full output")
  map("n", "<localleader>o", "<Plug>(ipynb-toggle-output)", "toggle full output")
  map("n", "<Plug>(ipynb-hide-output)", hide, "close full output")
  map("n", "<localleader>O", "<Plug>(ipynb-hide-output)", "close full output")
  map("n", "<localleader>x", open_rich_output, "open rich output")
  map("n", "<localleader>i", restart, "restart kernel")

  vim.b[bufnr].notebook_keymaps_set = true
end

local function activate_notebook_buffer(bufnr)
  if not is_ipynb_buffer(bufnr) then
    return
  end

  if vim.b[bufnr].ipynb_activation_tick == vim.api.nvim_buf_get_changedtick(bufnr)
    and vim.b[bufnr].ipynb_markdown_enabled == true then
    local embedded = vim.fn.bufnr(vim.api.nvim_buf_get_name(bufnr) .. ".otter.py")
    local keeper = package.loaded["otter.keeper"]
    local highlighter = vim.treesitter.highlighter.active[bufnr]
    if embedded >= 0 and vim.api.nvim_buf_is_loaded(embedded)
      and keeper and keeper.rafts[bufnr] and highlighter and highlighter.ipynb_fences then return end
  end

  vim.b[bufnr].notebook_markdown = true
  if vim.b[bufnr].completion == nil then vim.b[bufnr].completion = true end
  vim.bo[bufnr].swapfile = false
  require("ipynb.notebook_io").attach(bufnr)

  if not vim.api.nvim_buf_is_valid(bufnr) or not is_ipynb_buffer(bufnr) then
    return
  end

  ensure_notebook_rendering(bufnr)
  vim.api.nvim_buf_call(bufnr, function()
    local otter = require("otter")
    local embedded = vim.fn.bufnr(vim.api.nvim_buf_get_name(bufnr) .. ".otter.py")
    if embedded >= 0 and vim.api.nvim_buf_is_loaded(embedded) and require("otter.keeper").rafts[bufnr] then
      otter.sync_raft(bufnr)
    else
      otter.activate({ "python" }, true, true)
    end
  end)
  require("ipynb.diagnostics").attach(bufnr)
  require("ipynb.indent").attach(bufnr)

  set_notebook_keymaps(bufnr)
  if M.options.auto_init then ensure_notebook_kernel(bufnr) end
  vim.b[bufnr].ipynb_activation_tick = vim.api.nvim_buf_get_changedtick(bufnr)
end

function M.init(opts)
  if opts and opts.backend ~= nil and opts.backend ~= "rust" then
    error("The Python execution backend was removed. Remove opts.backend or set it to 'rust', then run :IpynbInstall.")
  end
  require("ipynb.install").path()
  local venv = vim.fn.stdpath("data") .. "/ipynb.nvim/venv"
  M.options = vim.tbl_deep_extend("force", {
    python = venv .. "/bin/python",
    images = true,
    auto_init = false,
    auto_setup = true,
    numpy_legacy_repr = true,
    keymaps = true,
    output = { preview_lines = 8 },
  }, opts or {})
  M.options.jupytext = M.options.jupytext or vim.fn.fnamemodify(M.options.python, ":h") .. "/jupytext"
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

local function install_runtime()
  local venv = vim.fn.stdpath("data") .. "/ipynb.nvim/venv"
  if M.options.python == venv .. "/bin/python" then
    local command = vim.fn.executable(M.options.python) == 1 and { M.options.python }
      or { "uv", "run", "--no-project", "--python", ">=3.11", "--" }
    vim.list_extend(command, { root .. "/scripts/setup.py", "--venv", venv })
    local result = vim.system(command, { text = true }):wait()
    if result.code ~= 0 then error(result.stderr .. result.stdout) end
  else
    local result = vim.system({ M.options.python, "-c",
      "import ipykernel, jupytext; from packaging.version import Version; assert Version(jupytext.__version__) >= Version('1.19.6'), 'Jupytext 1.19.6 or newer is required'",
    }, { text = true }):wait()
    if result.code ~= 0 then
      error("custom python runtime needs requirements.txt installed first: " .. (result.stderr or ""))
    end
  end
  local treesitter = require("nvim-treesitter")
  if treesitter.install then
    treesitter.install({ "markdown", "markdown_inline", "python" }):wait(300000)
    -- Lazy can omit this directory from runtimepath before the first install.
    vim.opt.runtimepath:prepend(require("nvim-treesitter.config").get_install_dir(""))
  else
    vim.cmd("TSInstallSync markdown markdown_inline python")
  end
  -- Refresh the runtime cache when installation creates a parser directory.
  vim.o.runtimepath = vim.o.runtimepath
  for _, language in ipairs({ "markdown", "markdown_inline", "python" }) do
    local ok, loaded, err = pcall(vim.treesitter.language.add, language)
    if not ok or not loaded then
      error("Notebook parser installation failed for " .. language .. ": " .. tostring(err or loaded))
    end
  end
  local result = vim.system({ "cargo", "build", "--release", "--locked" },
    { cwd = root .. "/native", text = true }):wait()
  if result.code ~= 0 then error("Rust engine build failed: " .. (result.stderr or result.stdout)) end
  vim.notify("notebook runtime installed. restart neovim before opening notebooks.")
end

function M.install()
  if not M.options then M.init() end
  return require("ipynb.install").run(root, M.options, install_runtime)
end

function M.setup(opts)
  M.init(opts)
  vim.filetype.add({ pattern = { [".*%.ipynb"] = function(_, buf)
    -- Quickfile detection must not replace the converted Markdown parser with JSON.
    if buf and buf >= 0 and vim.b[buf].notebook_markdown then return vim.bo[buf].filetype end
  end } })
  vim.api.nvim_create_user_command("IpynbInstall", M.install, { desc = "Check prerequisites and install notebook tools" })
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
  require("ipynb.jupytext").setup()
  require("ipynb.notebook_io").setup(require("jupytext"))
  local group = vim.api.nvim_create_augroup("ipynb", { clear = true })
  local function autocmd(events, callback)
    vim.api.nvim_create_autocmd(events, { group = group, pattern = "*.ipynb", callback = callback })
  end
  autocmd({ "BufReadPre", "BufNewFile" }, function(ev)
    vim.bo[ev.buf].swapfile = false
    vim.bo[ev.buf].buftype = "acwrite"
  end)
  autocmd({ "BufReadPost", "BufEnter" }, function(ev) activate_notebook_buffer(ev.buf) end)
  autocmd("BufWinEnter", function(ev)
    require("ipynb.markdown").enable(ev.buf)
    require("ipynb.native_ui").refresh(ev.buf)
  end)
  autocmd({ "BufWritePost", "InsertEnter", "InsertLeave", "TextChanged", "TextChangedI" }, function(ev)
    require("ipynb.markdown").refresh(ev.buf)
    require("ipynb.native_ui").refresh(ev.buf)
  end)
  autocmd("BufWritePre", function(ev) require("ipynb.editor").format(ev.buf) end)
  autocmd("BufWritePost", function(ev) require("ipynb.editor").lint(ev.buf) end)
  vim.api.nvim_create_autocmd({ "WinResized", "VimResized" }, { group = group, callback = function()
    -- image.nvim updates terminal cell pixels in this same event.
    vim.schedule(function()
      for _, win in ipairs(vim.api.nvim_list_wins()) do
        if vim.api.nvim_win_is_valid(win) then
          local buf = vim.api.nvim_win_get_buf(win)
          if is_ipynb_buffer(buf) then
            require("ipynb.markdown").refresh(buf)
            require("ipynb.native_ui").refresh(buf)
          end
        end
      end
    end)
  end })
  vim.api.nvim_create_autocmd("OptionSet", { group = group,
    pattern = { "number", "relativenumber", "numberwidth", "statuscolumn", "signcolumn", "foldcolumn" },
    callback = function()
      local buf = vim.api.nvim_get_current_buf()
      if not is_ipynb_buffer(buf) then return end
      vim.schedule(function()
        if not vim.api.nvim_buf_is_valid(buf) or vim.api.nvim_get_current_buf() ~= buf then return end
        -- Measure the gutter after Neovim applies the user's numbering change.
        vim.cmd("redraw")
        require("ipynb.markdown").refresh(buf)
        require("ipynb.native_ui").refresh(buf)
      end)
    end,
  })
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
  vim.api.nvim_create_autocmd("LspAttach", { group = group, callback = function(ev)
    local client = vim.lsp.get_client_by_id(ev.data.client_id)
    if not client then return end
    if client.name == "marksman" and is_ipynb_buffer(ev.buf) then
      vim.schedule(function()
        if vim.api.nvim_buf_is_valid(ev.buf) then vim.lsp.buf_detach_client(ev.buf, client.id) end
      end)
    elseif (client.name == "pyright" or client.name == "basedpyright") and is_ipynb_otter_buffer(ev.buf) then
      configure_notebook_python_lsp(client, ev.buf)
    end
  end })
end

return M
