local M = {}
local loaded, tmux = pcall(require, "image/utils/tmux")
local offset

if loaded and tmux.is_tmux and tmux.get_pane_tty then
  local get_tty, tty, queried = tmux.get_pane_tty, nil, false
  tmux.get_pane_tty = function()
    if not queried then
      tty, queried = get_tty(), true
      -- All clears in this batch target the same active pane; recheck next turn.
      vim.schedule(function() tty, queried = nil, false end)
    end
    return tty
  end
end

-- image.nvim already adds pane_left/pane_top. Passthrough coordinates also
-- include top status rows, which tmux omits from pane_top.
function M.top_offset()
  if not loaded or not tmux.is_tmux then return 0 end
  if offset ~= nil then return offset or nil end
  offset = false
  -- Share one snapshot across a render batch, then recheck slow callbacks.
  vim.schedule(function() offset = nil end)
  local pane = vim.env.TMUX_PANE
  if not pane or not pane:match("^%%%d+$") then return nil end
  local ok, result = pcall(vim.fn.system, {
    "tmux", "display-message", "-p", "-t", pane,
    "#{window_active} #{pane_active} #{==:#{client_session},#{session_name}} "
      .. "#{status-position} #{status} #{client_height} #{client_control_mode}",
  })
  if not ok or vim.v.shell_error ~= 0 then return nil end
  local window, active, session, position, status, height, control = result:match(
    "^(%d+) (%d+) (%d+) (%a+) (%w+) (%d+) (%d+)%s*$"
  )
  if window ~= "1" or active ~= "1" or session ~= "1" or control ~= "0" then return nil end
  local rows = status == "on" and 1 or status == "off" and 0 or tonumber(status)
  if not rows or rows < 0 or rows > 5 or (position ~= "top" and position ~= "bottom") then return nil end
  -- tmux suppresses status when the client cannot fit its configured rows.
  offset = position == "top" and tonumber(height) > rows and rows or 0
  return offset
end

return M
