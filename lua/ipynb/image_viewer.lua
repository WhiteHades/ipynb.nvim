local image_api = require("ipynb.molten_load_image_nvim").image_api

local M = {}
local active
local next_id = 0
local autocmd_group

local function limit(value)
  return math.max(1, math.floor(value * 0.92) - 2)
end

local function config(width, height)
  local width_total, height_total = width + 2, height + 2
  return {
    relative = "editor",
    row = math.max(0, math.floor((vim.o.lines - height_total) / 2)),
    col = math.max(0, math.floor((vim.o.columns - width_total) / 2)),
    width = width,
    height = height,
    border = "rounded",
    title = " Ipynb Image ",
    title_pos = "center",
    style = "minimal",
    focusable = true,
    zindex = 100,
  }
end

local function clear_autocmds()
  if autocmd_group then
    vim.api.nvim_clear_autocmds({ group = autocmd_group })
    autocmd_group = nil
  end
end

local function close()
  local viewer = active
  if not viewer then return end
  active = nil
  clear_autocmds()

  if viewer.identifier then image_api.clear(viewer.identifier) end
  if vim.api.nvim_win_is_valid(viewer.window) then
    vim.cmd(("noautocmd call nvim_win_close(%d, v:true)"):format(viewer.window))
  end
  if vim.api.nvim_buf_is_valid(viewer.buffer) then
    vim.cmd(("noautocmd call nvim_buf_delete(%d, {'force': v:true})"):format(viewer.buffer))
  end
  image_api.refresh()
end

local function add_image(viewer, max_width, max_height)
  local ok, identifier = pcall(image_api.from_file, viewer.path, {
    id = viewer.id,
    buffer = viewer.buffer,
    window = viewer.window,
    x = 0,
    y = 0,
    max_width = max_width,
    max_height = max_height,
    viewer = true,
  })
  if not ok or not identifier then return nil end
  viewer.identifier = identifier
  return identifier
end

local function fill_buffer(buffer, height)
  local lines = {}
  for _ = 1, height do lines[#lines + 1] = "" end
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines)
end

local function relayout()
  local viewer = active
  if not viewer or not vim.api.nvim_win_is_valid(viewer.window) then
    close()
    return false
  end

  local max_width, max_height = limit(vim.o.columns), limit(vim.o.lines)
  if viewer.identifier then image_api.clear(viewer.identifier) end
  -- Kitty may round a fractional pixel height up to the next terminal row.
  -- Scale against one fewer row and keep that row inside the popup as padding.
  local identifier = add_image(viewer, max_width, math.max(1, max_height - 1))
  if not identifier then
    close()
    return false
  end

  local ok, size = pcall(image_api.image_size, identifier)
  local image_width = type(size) == "table" and tonumber(size.width)
  local image_height = type(size) == "table" and tonumber(size.height)
  if not ok or not image_width or not image_height or image_width <= 0 or image_height <= 0 then
    close()
    return false
  end
  local width = math.min(image_width, max_width)
  local height = math.min(image_height + 1, max_height)
  fill_buffer(viewer.buffer, height)
  vim.api.nvim_win_set_config(viewer.window, config(width, height))
  image_api.render(identifier)
  image_api.refresh()
  return true
end

local function setup_autocmds(viewer)
  autocmd_group = vim.api.nvim_create_augroup("IpynbImageViewer", { clear = true })
  local function current()
    return active == viewer
  end

  vim.api.nvim_create_autocmd({ "BufWipeout", "WinLeave" }, {
    group = autocmd_group,
    buffer = viewer.buffer,
    callback = function()
      if current() then close() end
    end,
  })
  vim.api.nvim_create_autocmd("WinClosed", {
    group = autocmd_group,
    pattern = tostring(viewer.window),
    callback = function()
      if current() then close() end
    end,
  })
  vim.api.nvim_create_autocmd("VimResized", {
    group = autocmd_group,
    callback = function()
      if current() then relayout() end
    end,
  })
end

function M.open(path)
  if type(path) ~= "string" or path == "" then return false end
  close()

  local buffer = vim.api.nvim_create_buf(false, true)
  vim.bo[buffer].filetype = "ipynb_output"
  vim.bo[buffer].bufhidden = "wipe"
  local ok, window = pcall(vim.api.nvim_open_win, buffer, false, config(1, 1))
  if not ok then
    vim.api.nvim_buf_delete(buffer, { force = true })
    vim.notify("Unable to display image", vim.log.levels.WARN)
    return false
  end
  vim.cmd(("noautocmd call nvim_set_current_win(%d)"):format(window))

  next_id = next_id + 1
  active = { path = path, id = ("ipynb-image-viewer-%d"):format(next_id), buffer = buffer, window = window }
  vim.keymap.set("n", "q", close, { buffer = buffer, silent = true, nowait = true })
  vim.keymap.set("n", "<Esc>", close, { buffer = buffer, silent = true, nowait = true })
  setup_autocmds(active)
  local opened = relayout()
  if not opened then vim.notify("Unable to display image", vim.log.levels.WARN) end
  return opened
end

M.close = close

return M
