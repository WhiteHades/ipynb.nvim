local M = {}
local namespace = vim.api.nvim_create_namespace("ipynb-cell-header")
local frames = { "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏" }
local pending = false

function M.clear(buf)
  if vim.api.nvim_buf_is_valid(buf) then vim.api.nvim_buf_clear_namespace(buf, namespace, 0, -1) end
end

function M.refresh(buf)
  M.clear(buf)
  if not vim.api.nvim_buf_is_valid(buf) or vim.b[buf].ipynb_markdown_enabled ~= true then return end
  local wins = vim.fn.win_findbuf(buf)
  local width
  for _, win in ipairs(wins) do
    local info = vim.fn.getwininfo(win)[1]
    local available = info.width - info.textoff
    width = width and math.min(width, available) or available
  end
  if not width or width < 20 then return end
  local animate = vim.g.ipynb_animations ~= false and vim.g.snacks_animate ~= false and #vim.api.nvim_list_uis() > 0
  local busy = false
  for index, cell in ipairs(require("ipynb.native_ui").cell_headers(buf)) do
    local row = cell.row
    local fence = row >= 0 and (vim.api.nvim_buf_get_lines(buf, row, row + 1, false)[1] or "") or ""
    if row >= 0 and fence:match("^```") then
      local count = type(cell.execution_count) == "number" and tostring(cell.execution_count) or " "
      local label = cell.status == "running" and "Running" or cell.status == "hold" and "Queued"
        or cell.status == "new" and "Ready" or cell.success == false and "Failed"
        or cell.old and "Previous" or cell.status == "done" and "Done" or "Ready"
      local visible = false
      for _, win in ipairs(wins) do if vim.fn.screenpos(win, row + 1, 1).row > 0 then visible = true; break end end
      local moving = label == "Running" and animate and visible
      if moving then
        label = frames[math.floor(vim.uv.hrtime() / 120000000) % #frames + 1] .. " " .. label
      end
      local edge = require("ipynb.panels").available() and "   " or " ─╮"
      local text = (" Cell %d  In[%s]  %s%s"):format(index, count, label, edge)
      if vim.fn.strdisplaywidth(text) > width - 12 then text = (" %d  In[%s]%s"):format(index, count, edge) end
      if vim.fn.strdisplaywidth(text) > width - 12 then text = (" %d%s"):format(index, edge) end
      busy = busy or (moving and text:find("Running", 1, true) ~= nil)
      vim.api.nvim_buf_set_extmark(buf, namespace, row, 0, {
        virt_text = { { text, cell.group } }, virt_text_pos = "right_align", priority = 310,
      })
    end
  end
  if busy and not pending then
    pending = true
    vim.defer_fn(function()
      pending = false
      local seen = {}
      for _, win in ipairs(vim.api.nvim_list_wins()) do
        local displayed = vim.api.nvim_win_get_buf(win)
        if not seen[displayed] and vim.b[displayed].ipynb_markdown_enabled == true then
          seen[displayed] = true
          M.refresh(displayed)
        end
      end
    end, 120)
  end
end

return M
