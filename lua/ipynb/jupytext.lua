local M = {}

local actions = { "menu", "info", "export", "pair", "unpair", "sync", "pipe", "check", "cli" }
local labels = {
  info = "Formats, pairs, and configuration", export = "Export to another format",
  pair = "Set paired formats", unpair = "Remove pairing, keep files", sync = "Sync paired files",
  pipe = "Pipe through a command and review changes", check = "Check with an external command",
  cli = "Advanced Jupytext CLI",
}

local function notify(message, level)
  vim.notify(message, level or vim.log.levels.INFO, { title = "Jupytext" })
end

local function relative(path)
  return vim.fn.fnamemodify(path, ":~:.")
end

local function unchanged(ctx)
  if vim.api.nvim_buf_is_valid(ctx.buf)
    and vim.api.nvim_buf_get_name(ctx.buf) == ctx.path
    and vim.api.nvim_buf_get_changedtick(ctx.buf) == ctx.tick
    and (vim.api.nvim_get_current_buf() == ctx.buf or vim.api.nvim_get_current_buf() == ctx.preview) then
    return true
  end
  notify("Notebook changed or focus moved. Run the action again. Any completed disk writes remain on disk.", vim.log.levels.WARN)
  return false
end

local function select(ctx, items, opts, callback)
  vim.ui.select(items, opts, function(item)
    if item ~= nil and unchanged(ctx) then callback(item) end
  end)
end

local function input(ctx, opts, callback)
  vim.ui.input(opts, function(value)
    if value ~= nil and value:match("%S") and unchanged(ctx) then callback(value) end
  end)
end

local function request(ctx, action, extra)
  if not unchanged(ctx) then return end
  local lines = vim.api.nvim_buf_get_lines(ctx.buf, 0, -1, false)
  local params = vim.tbl_extend("force", {
    action = action, buf = ctx.buf, path = ctx.path, lines = lines,
    text = table.concat(lines, "\n"), spans = require("ipynb.cells").spans(ctx.buf),
    positions = require("ipynb.native_ui").positions(ctx.buf), mtime = ctx.mtime,
  }, extra or {})
  local ok, result = pcall(require("ipynb.native").request, "jupytext", params)
  if not ok then notify(tostring(result), vim.log.levels.ERROR); return end
  if not unchanged(ctx) then return end
  return result
end

local function float(lines, title, filetype)
  local buf = vim.api.nvim_create_buf(false, true)
  local expanded = {}
  for _, line in ipairs(lines) do vim.list_extend(expanded, vim.split(line, "\n", { plain = true })) end
  lines = expanded
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].bufhidden, vim.bo[buf].swapfile = "wipe", false
  vim.bo[buf].filetype = filetype or "text"
  vim.bo[buf].modifiable, vim.bo[buf].readonly = false, true
  local width = math.max(1, math.min(100, vim.o.columns - 4))
  local height = math.max(1, math.min(#lines, math.floor(vim.o.lines * 0.75)))
  local win = vim.api.nvim_open_win(buf, true, {
    relative = "editor", style = "minimal", border = "rounded", title = " " .. title .. " ",
    width = width, height = height, row = math.max(0, math.floor((vim.o.lines - height) / 2) - 1),
    col = math.max(0, math.floor((vim.o.columns - width) / 2)),
  })
  vim.wo[win].wrap = false
  local function close()
    if vim.api.nvim_win_is_valid(win) then vim.api.nvim_win_close(win, true) end
  end
  for _, key in ipairs({ "q", "<Esc>" }) do
    vim.keymap.set("n", key, close, { buffer = buf, silent = true, desc = "Close Jupytext view" })
  end
  return buf, close
end

local function format_label(item)
  local label = item.format .. "  " .. (type(item.name) == "string" and item.name or "")
  if item.available == false then
    local needs = type(item.requires) == "table" and table.concat(item.requires, ", ") or item.requires
    label = label .. " [unavailable" .. (needs and needs ~= "" and ": requires " .. needs or "") .. "]"
  end
  return label
end

local function pair_format(fmt)
  if type(fmt) == "string" then return fmt end
  if type(fmt) ~= "table" then return "unknown" end
  return (fmt.extension or ""):gsub("^%.", "")
    .. (type(fmt.format_name) == "string" and ":" .. fmt.format_name or "")
end

local function show_info(ctx)
  local result = request(ctx, "info")
  if not result then return end
  local lines = { "Jupytext " .. tostring(result.version or "unknown"), "", "Configuration" }
  local config = result.config
  if type(config) == "string" then
    lines[#lines + 1] = relative(config)
  elseif type(config) == "table" then
    if type(config.path) == "string" then lines[#lines + 1] = relative(config.path) end
    -- Only show Jupytext settings, never arbitrary config values or environment variables.
    for _, key in ipairs({ "formats", "default_jupytext_formats", "preferred_jupytext_formats",
      "notebook_metadata_filter", "cell_metadata_filter", "comment_magics", "split_at_heading" }) do
      if config[key] ~= nil then lines[#lines + 1] = key .. " = " .. vim.inspect(config[key]) end
    end
  else
    lines[#lines + 1] = "Jupytext defaults"
  end
  lines[#lines + 1] = ""
  lines[#lines + 1] = "Paired files"
  for _, pair in ipairs(result.pairs or {}) do
    lines[#lines + 1] = relative(pair.path) .. "  [" .. pair_format(pair.fmt) .. "]"
  end
  if #(result.pairs or {}) == 0 then lines[#lines + 1] = "No paired files" end
  if #(result.changed or {}) > 0 then
    lines[#lines + 1] = ""
    lines[#lines + 1] = "Changed on disk"
    for _, path in ipairs(result.changed) do lines[#lines + 1] = relative(path) end
  end
  lines[#lines + 1] = ""
  lines[#lines + 1] = "Supported formats"
  for _, item in ipairs(result.formats or {}) do lines[#lines + 1] = format_label(item) end
  lines[#lines + 1] = ""
  lines[#lines + 1] = "q / Esc: close"
  float(lines, "Jupytext")
end

local function written(result)
  local paths = result.paths or result.writes or {}
  local names = {}
  for _, path in ipairs(paths) do names[#names + 1] = relative(path) end
  notify(#names > 0 and "Saved " .. table.concat(names, ", ") or "Jupytext action completed")
end

local function persist(ctx, action, params)
  local result = request(ctx, action, params)
  if not result then return end
  if type(result.text) ~= "string" or type(result.mtime) ~= "table" then
    notify("Jupytext wrote files but returned no notebook snapshot. Reload the notebook before saving.", vim.log.levels.ERROR)
    return
  end
  local lines = vim.split(result.text, "\n", { plain = true })
  local changed = table.concat(vim.api.nvim_buf_get_lines(ctx.buf, 0, -1, false), "\n") ~= result.text
  if changed then vim.api.nvim_buf_set_lines(ctx.buf, 0, -1, false, lines) end
  vim.b[ctx.buf].mtime = result.mtime
  local metadata = result.metadata or {}
  vim.b[ctx.buf].ipynb_read_kernel = (metadata.kernelspec or {}).name or false
  vim.b[ctx.buf].ipynb_language = (metadata.language_info or {}).name or (metadata.kernelspec or {}).language
  local native = require("ipynb.native")
  if #native.running(ctx.buf) == 0 then vim.b[ctx.buf].ipynb_kernel_selection = nil end
  vim.bo[ctx.buf].modified = false
  require("ipynb.native_ui").clear(ctx.buf)
  local ok, err = pcall(native.request, "preview", {
    buf = ctx.buf, path = ctx.path, lines = lines, spans = require("ipynb.cells").spans(ctx.buf),
  })
  if not ok then notify("Saved notebook, but could not refresh outputs: " .. tostring(err), vim.log.levels.WARN) end
  written(result)
end

local function export(ctx)
  local info = request(ctx, "info")
  if not info then return end
  select(ctx, info.formats or {}, { prompt = "Export format", format_item = format_label }, function(item)
    if item.available == false then notify(format_label(item), vim.log.levels.WARN); return end
    local extension = item.extension or "." .. item.format:match("^[^:]+")
    if extension:sub(1, 1) ~= "." then extension = "." .. extension end
    input(ctx, { prompt = "Export path: ", default = ctx.path:gsub("%.ipynb$", "") .. extension, completion = "file" }, function(target)
      target = vim.fn.fnamemodify(vim.fs.normalize(target), ":p")
      local function save(overwrite)
        local result = request(ctx, "export", { fmt = item.format, target = target, overwrite = overwrite })
        if result then written(result) end
      end
      if vim.uv.fs_lstat(target) then
        select(ctx, { "Cancel", "Overwrite " .. relative(target) }, { prompt = "Target exists. Overwrite?" }, function(choice)
          if choice ~= "Cancel" then save(true) end
        end)
      else
        save(false)
      end
    end)
  end)
end

local function sync(ctx)
  local info = request(ctx, "info")
  if not info then return end
  local choices = { { label = "Current editor buffer", source = "buffer" } }
  for _, pair in ipairs(info.pairs or {}) do
    choices[#choices + 1] = { label = relative(pair.path) .. " [" .. pair_format(pair.fmt) .. "]", source = pair.path }
  end
  local prompt = "Sync from which source? Other paired files will be updated."
  if vim.bo[ctx.buf].modified or #(info.changed or {}) > 0 then
    prompt = "Unsaved or external changes: choose the source to keep"
  end
  select(ctx, choices, { prompt = prompt, format_item = function(item) return item.label end }, function(item)
    persist(ctx, "sync", { source = item.source })
  end)
end

local function external(ctx, action)
  input(ctx, { prompt = action == "pipe" and "Pipe command: " or "Check command: " }, function(command)
    select(ctx, { "Cancel", "Run command" }, { prompt = "Execute external command: " .. command }, function(choice)
      if choice == "Cancel" then return end
      local result = request(ctx, action, { command = command })
      if not result then return end
      if action == "check" then
        notify(result.notice or result.message or "Jupytext check completed")
        return
      end
      if type(result.text) ~= "string" then notify("Pipe returned no proposed text", vim.log.levels.ERROR); return end
      local before = table.concat(vim.api.nvim_buf_get_lines(ctx.buf, 0, -1, false), "\n")
      if before == result.text then notify("Pipe produced no text changes"); return end
      local diff = vim.diff(before .. "\n", result.text .. "\n", { result_type = "unified", ctxlen = 3 })
      local close
      ctx.preview, close = float(vim.split(diff, "\n", { plain = true }), "Pipe changes, disk unchanged", "diff")
      vim.ui.select({ "Cancel", "Apply to editor buffer" }, { prompt = "Apply the reviewed pipe changes?" }, function(apply)
        local valid = apply == "Apply to editor buffer" and unchanged(ctx)
        close()
        ctx.preview = nil
        if valid then
          vim.api.nvim_buf_set_lines(ctx.buf, 0, -1, false, vim.split(result.text, "\n", { plain = true }))
          require("ipynb.native_ui").clear(ctx.buf)
          local refreshed, err = pcall(require("ipynb.native").request, "preview", {
            buf = ctx.buf, path = ctx.path, notebook = result.notebook, reset = true,
            lines = vim.split(result.text, "\n", { plain = true }), spans = require("ipynb.cells").spans(ctx.buf),
          })
          if not refreshed then notify("Could not restore pipe output previews: " .. tostring(err), vim.log.levels.WARN) end
          vim.bo[ctx.buf].modified = true
          notify("Applied pipe changes to the editor buffer. Disk is unchanged.")
        end
      end)
    end)
  end)
end

local function cli(ctx)
  select(ctx, { "Cancel", "Open terminal" }, {
    prompt = "Advanced CLI writes bypass editor guards. Save or sync first, then reload after CLI writes.",
  }, function(choice)
    if choice == "Cancel" then return end
    local python = require("ipynb").options.python
    if python:find("%c") or ctx.path:find("%c") then
      notify("Cannot prefill terminal commands for paths containing control characters", vim.log.levels.ERROR)
      return
    end
    vim.cmd("botright new")
    local channel = vim.fn.jobstart({ vim.o.shell }, { term = true, cwd = vim.fn.fnamemodify(ctx.path, ":h") })
    if channel <= 0 then notify("Could not open the Jupytext terminal", vim.log.levels.ERROR); return end
    -- Prefill only. The user edits the command and presses Enter to execute it.
    vim.api.nvim_chan_send(channel, vim.fn.shellescape(python) .. " -m jupytext " .. vim.fn.shellescape(ctx.path) .. " ")
    vim.cmd("startinsert")
  end)
end

function M.run(action)
  action = action and action ~= "" and action or "menu"
  if not vim.tbl_contains(actions, action) then notify("Unknown Jupytext action: " .. action, vim.log.levels.ERROR); return end
  local buf = vim.api.nvim_get_current_buf()
  local path = vim.api.nvim_buf_get_name(buf)
  if not path:match("%.ipynb$") or path:find("://", 1, true) then
    notify("Open a local .ipynb notebook first", vim.log.levels.WARN)
    return
  end
  local ctx = { buf = buf, path = path, tick = vim.api.nvim_buf_get_changedtick(buf), mtime = vim.b[buf].mtime }
  local function dispatch(selected)
    if selected == "info" then show_info(ctx)
    elseif selected == "export" then export(ctx)
    elseif selected == "pair" then
      input(ctx, { prompt = "Paired formats, including directory prefixes: ", default = "ipynb,py:percent" }, function(formats)
        select(ctx, { "Cancel", "Pair without overwriting existing files", "Pair and overwrite existing files" }, {
          prompt = "Pair formats: " .. formats,
        }, function(choice)
          if choice ~= "Cancel" then
            persist(ctx, "pair", { formats = formats, overwrite = choice == "Pair and overwrite existing files" })
          end
        end)
      end)
    elseif selected == "unpair" then
      select(ctx, { "Cancel", "Remove pairing and retain all files" }, { prompt = "Remove pairing? Files will not be deleted." }, function(choice)
        if choice ~= "Cancel" then persist(ctx, "unpair") end
      end)
    elseif selected == "sync" then sync(ctx)
    elseif selected == "pipe" or selected == "check" then external(ctx, selected)
    elseif selected == "cli" then cli(ctx) end
  end
  if action == "menu" then
    local items = vim.list_slice(actions, 2)
    select(ctx, items, { prompt = "Jupytext formats and sync", format_item = function(item) return labels[item] end }, dispatch)
  else
    dispatch(action)
  end
end

function M.setup()
  vim.api.nvim_create_user_command("IpynbJupytext", function(opts) M.run(opts.args) end, {
    nargs = "?", desc = "Jupytext formats, pairing, sync, and commands",
    complete = function(prefix)
      return vim.tbl_filter(function(action) return action:sub(1, #prefix) == prefix end, actions)
    end,
  })
end

return M
