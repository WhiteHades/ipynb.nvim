-- Docker/SSH may omit pixel dimensions from the PTY. The terminal still knows
-- its real cell size: request the measured cell rectangle explicitly instead
-- of relying on the resized bitmap's pixel dimensions for its display size.
local M = {}
local images = setmetatable({}, { __mode = "v" })
local attached = false
local warned = false
local finishing = false
local transport_pending = false

local function ui_transport_available()
  if type(vim.api.nvim_ui_send) ~= "function" then return false end
  for _, ui in ipairs(vim.api.nvim_list_uis()) do
    if ui.stdout_tty then return true end
  end
  return false
end

local function control_sequence(config)
  local codes = require("image/backends/kitty/codes").control.keys
  local control = {}
  for key, value in pairs(config) do
    local code = codes[key]
    if code and value ~= nil then
      if type(value) == "number" then value = string.format("%d", value) end
      control[#control + 1] = code .. "=" .. value
    end
  end
  return "\x1b_G" .. table.concat(control, ",") .. "\x1b\\"
end

local function send(sequence, tmux)
  if tmux.is_tmux then sequence = tmux.escape(sequence) end
  vim.api.nvim_ui_send(sequence)
end

function M.track(img, term)
  if not img.internal_id or img.global_state.options.backend ~= "kitty" then return end
  images[img.internal_id] = img
  if attached then return end
  local loaded, helpers = pcall(require, "image/backends/kitty/helpers")
  if not loaded or type(helpers) ~= "table" or type(helpers.write_graphics_at) ~= "function" or type(helpers.write_graphics) ~= "function" or type(helpers.update_sync_end) ~= "function" then
    if not warned then
      warned = true
      vim.notify("ipynb.nvim: image.nvim's Kitty API changed. Update both plugins; notebook image adjustments are disabled.", vim.log.levels.WARN)
    end
    return
  end
  local tmux = require("image/utils/tmux")
  local image_tmux = require("ipynb.image_tmux")
  local write = helpers.write_graphics_at
  local write_graphics = helpers.write_graphics
  local sync_end = helpers.update_sync_end
  helpers.update_sync_end = function(...)
    if transport_pending then
      transport_pending = false
      if ui_transport_available() then
        send("\x1b[?2026l", tmux)
        return
      end
    end
    return sync_end(...)
  end
  helpers.write_graphics_at = function(config, x, y)
    local img = images[config.image_id]
    if img and config.action == "p" then
      if img.global_state.disable_decorator_handling then return end
      local offset = image_tmux.top_offset()
      if offset == nil then return end
      y = y + offset
      -- Keep notebook graphics above opaque cell backgrounds in Ghostty.
      config = vim.tbl_extend("force", config, { display_zindex = 1 })
      if config.display_width and config.display_height then
        local size = term.get_size()
        if size and size.cell_width > 0 and size.cell_height > 0 then
          config = vim.tbl_extend("force", config, {
            display_columns = math.max(1, math.ceil(config.display_width / size.cell_width)),
            display_rows = math.max(1, math.ceil(config.display_height / size.cell_height)),
          })
        end
      end
    end
    local result
    if img and config.action == "p" and config.tty == nil and ui_transport_available() then
      if tmux.is_tmux then
        local position = tmux.get_pane_position()
        x, y = x + position.left, y + position.top
      end
      send("\x1b[?2026h\x1b[s\x1b[" .. y .. ";" .. x .. "H" .. control_sequence(config) .. "\x1b[u\x1b[?2026l", tmux)
      transport_pending = true
    else
      result = write(config, x, y)
    end
    if images[config.image_id] and config.action == "p" and not finishing then
      finishing = true
      -- Finish after queued TUI writes, including cold asynchronous placements.
      vim.schedule(function()
        finishing = false
        helpers.update_sync_end()
      end)
    end
    return result
  end
  helpers.write_graphics = function(config, data, chunk_size)
    local img = images[config.image_id]
    if not img or config.tty ~= nil or not ui_transport_available() then
      return write_graphics(config, data, chunk_size)
    end
    if not data then
      send(control_sequence(config), tmux)
      return
    end
    if config.transmit_medium ~= require("image/backends/kitty/codes").control.transmit_medium.file then
      return write_graphics(config, data, chunk_size)
    end
    local payload = control_sequence(config):sub(4, -3)
    local encoded = vim.base64.encode(data):gsub("%-", "/")
    local size = type(chunk_size) == "number" and chunk_size > 0 and chunk_size or 4096
    local chunks = {}
    for i = 1, #encoded, size do chunks[#chunks + 1] = encoded:sub(i, i + size - 1):gsub("%s", "") end
    payload = payload .. ",m=" .. (#chunks > 1 and "1" or "0")
    for i, chunk in ipairs(chunks) do
      send("\x1b_G" .. payload .. ";" .. chunk .. "\x1b\\", tmux)
      payload = i == #chunks - 1 and "m=0" or "m=1"
    end
  end
  attached = true
end

return M
