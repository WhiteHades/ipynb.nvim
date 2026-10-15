-- Docker/SSH may omit pixel dimensions from the PTY. The terminal still knows
-- its real cell size: request the measured cell rectangle explicitly instead
-- of relying on the resized bitmap's pixel dimensions for its display size.
local M = {}
local images = setmetatable({}, { __mode = "v" })
local attached = false
local warned = false
local finishing = false

function M.track(img, term)
  if not img.internal_id or img.global_state.options.backend ~= "kitty" then return end
  images[img.internal_id] = img
  if attached then return end
  local loaded, helpers = pcall(require, "image/backends/kitty/helpers")
  if not loaded or type(helpers) ~= "table" or type(helpers.write_graphics_at) ~= "function" or type(helpers.update_sync_end) ~= "function" then
    if not warned then
      warned = true
      vim.notify("ipynb.nvim: image.nvim's Kitty API changed. Update both plugins; notebook image adjustments are disabled.", vim.log.levels.WARN)
    end
    return
  end
  local tmux = require("ipynb.image_tmux")
  local write = helpers.write_graphics_at
  helpers.write_graphics_at = function(config, x, y)
    local img = images[config.image_id]
    if img and config.action == "p" then
      if img.global_state.disable_decorator_handling then return end
      local offset = tmux.top_offset()
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
    local result = write(config, x, y)
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
  attached = true
end

return M
