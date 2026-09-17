-- Terminal-reported cell pixels take precedence over ioctl estimates, notably
-- when Docker/SSH transports only the terminal's row and column counts.
-- The TUI must forward CSI 6 replies to TermResponse. Neovim 0.12.5 discards
-- them; on that frontend this retains the provider's existing ioctl fallback.
local M = {}

function M.attach(term, changed)
  local original_size = term.get_size
  local cell_width, cell_height
  local group = vim.api.nvim_create_augroup("ipynb_image_terminal", { clear = true })

  term.get_size = function()
    local size = original_size()
    if not cell_width then return size end
    -- Never modify image.nvim's cached table: its resize handler still owns it.
    return vim.tbl_extend("force", size or {}, {
      cell_width = cell_width,
      cell_height = cell_height,
      screen_cols = vim.o.columns,
      screen_rows = vim.o.lines,
      screen_x = vim.o.columns * cell_width,
      screen_y = vim.o.lines * cell_height,
    })
  end

  vim.api.nvim_create_autocmd("TermResponse", {
    group = group,
    callback = function(event)
      local response = event.data and event.data.sequence or vim.v.termresponse
      if type(response) ~= "string" then return end
      local height, width = response:match("^\27%[6;(%d+);(%d+)t$")
      height, width = tonumber(height), tonumber(width)
      if not height or not width or height <= 0 or width <= 0
        or height == math.huge or width == math.huge then return end
      if cell_width == width and cell_height == height then return end
      cell_width, cell_height = width, height
      vim.schedule(changed)
    end,
  })

  local function query()
    if #vim.api.nvim_list_uis() == 0 then return end
    -- nvim_ui_send writes to the attached TUI, not the remote Python host's
    -- stdout, and leaves terminal input ownership with Neovim.
    if vim.api.nvim_ui_send then vim.api.nvim_ui_send("\27[16t") end
  end
  vim.api.nvim_create_autocmd({ "UIEnter", "VimResized", "FocusGained" }, {
    group = group, callback = function() vim.schedule(query) end,
  })
  vim.schedule(query)
  return term
end

return M
