local M = {}
local loaded, tmux = pcall(require, "image/utils/tmux")
local offset
local recipients, live_clients
local hooks = {
  "session-window-changed", "client-session-changed", "after-select-pane",
  "after-set-option", "after-resize-pane", "client-resized", "client-detached", "client-attached",
}
local watcher, leave_autocmd
local uv = vim.uv or vim.loop

local function shell_quote(value)
  return "'" .. value:gsub("'", "'\\''") .. "'"
end

local function tmux_quote(value)
  return '"' .. value:gsub("\\", "\\\\"):gsub('"', '\\"'):gsub("#", "##") .. '"'
end

local function tmux_call(args)
  local ok, output = pcall(vim.fn.system, args)
  if not ok or vim.v.shell_error ~= 0 then return nil end
  return output
end

local function owned_hooks(output, nonce)
  local found = {}
  for line in output:gmatch("[^\r\n]+") do
    local event, index, body = line:match("^([%w%-]+)%[(%d+)%]%s+(.*)$")
    if event and body:find(nonce, 1, true) then found[#found + 1] = { event, index } end
  end
  return found
end

local function remove_hooks(nonce)
  local output = tmux_call({ "tmux", "show-hooks", "-g" })
  if not output then return end
  local entries, args = owned_hooks(output, nonce), { "tmux" }
  for i, entry in ipairs(entries) do
    if i > 1 then args[#args + 1] = ";" end
    args[#args + 1] = "set-hook"
    args[#args + 1] = "-gu"
    args[#args + 1] = entry[1] .. "[" .. entry[2] .. "]"
  end
  if #entries > 0 then tmux_call(args) end
end

local function remove_files(state)
  if state.watcher then state.watcher:stop(); state.watcher:close() end
  if state.script_tmp then uv.fs_unlink(state.script_tmp) end
  if state.script then uv.fs_unlink(state.script) end
  if state.signal then uv.fs_unlink(state.signal) end
  if state.dir then uv.fs_rmdir(state.dir) end
end

local function cleanup_script(state, indices)
  local lines = {
    "#!/bin/sh",
    "signal=" .. shell_quote(state.signal),
    "pid=" .. shell_quote(tostring(uv.os_getpid())),
    "nonce=" .. shell_quote(state.nonce),
    'if kill -0 "$pid" 2>/dev/null; then touch "$signal" 2>/dev/null || :; exit 0; fi',
    'hooks=$(tmux show-hooks -g 2>/dev/null) || hooks=',
    'printf \'%s\\n\' "$hooks" | while IFS= read -r line; do',
    '  case "$line" in',
  }
  for _, event in ipairs(hooks) do
    local target = event .. "[" .. indices[event] .. "]"
    lines[#lines + 1] = "    " .. shell_quote(target .. " ") .. '*"$nonce"*) tmux set-hook -gu '
      .. shell_quote(target) .. " >/dev/null 2>&1 || : ;;"
  end
  lines[#lines + 1] = "  esac"
  lines[#lines + 1] = "done"
  lines[#lines + 1] = "rm -f " .. shell_quote(state.script) .. " " .. shell_quote(state.signal)
  lines[#lines + 1] = "rmdir " .. shell_quote(state.dir) .. " 2>/dev/null || :"
  return table.concat(lines, "\n") .. "\n"
end

local function write_script(state, contents)
  local fd, err = uv.fs_open(state.script_tmp, "w", 384)
  if not fd then return false, err end
  local wrote, write_err = uv.fs_write(fd, contents, 0)
  local closed, close_err = uv.fs_close(fd)
  if wrote ~= #contents then return false, write_err end
  if not closed then return false, close_err end
  local chmod_ok = uv.fs_chmod(state.script_tmp, 448)
  if not chmod_ok then return false end
  local rename_ok = uv.fs_rename(state.script_tmp, state.script)
  return not not rename_ok
end

local function signal_command(script)
  local shell = "if test -x " .. shell_quote(script) .. "; then exec " .. shell_quote(script) .. "; fi"
  return "run-shell -b " .. tmux_quote(shell)
end

local client_format = "#{client_pid} #{client_created} #{client_tty} #{window_id} #{client_control_mode}"

local function parse_clients(output, window)
  local result = {}
  for line in output:gmatch("[^\r\n]+") do
    local pid, created, tty, current, control = line:match("^(%d+) (%d+) (/dev/[%w/_.%-]+) (@%d+) (%d+)$")
    if tty and control == "0" and (not window or current == window) then
      result[pid .. ":" .. created .. ":" .. tty] = tty
    end
  end
  return result
end

function M.visible_clients()
  if not loaded or not tmux.is_tmux then return nil end
  if recipients ~= nil then return recipients end
  local pane = vim.env.TMUX_PANE
  if not pane or not pane:match("^%%%d+$") then return nil end
  local output = tmux_call({ "tmux", "display-message", "-p", "-t", pane, "#{window_id}",
    ";", "list-clients", "-F", client_format })
  local window = output and output:match("^(@%d+)\r?\n")
  if window then recipients = parse_clients(output, window) end
  return recipients
end

function M.live_clients()
  if not loaded or not tmux.is_tmux then return nil end
  if live_clients ~= nil then return live_clients end
  local output = tmux_call({ "tmux", "list-clients", "-F", client_format })
  if output then
    live_clients = parse_clients(output)
    vim.schedule(function() live_clients = nil end)
  end
  return live_clients
end

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
      .. "#{status-position} #{status} #{client_height} #{client_control_mode} "
      .. "#{window_zoomed_flag} #{pane_zoomed_flag}",
  })
  if not ok or vim.v.shell_error ~= 0 then return nil end
  local window, _, session, position, status, height, control, zoomed, pane_zoomed = result:match(
    "^(%d+) (%d+) (%d+) (%a+) (%w+) (%d+) (%d+) (%d+) (%d+)%s*$"
  )
  if window ~= "1" or session ~= "1" or control ~= "0"
    or (zoomed ~= "0" and pane_zoomed ~= "1") then return nil end
  local rows = status == "on" and 1 or status == "off" and 0 or tonumber(status)
  if not rows or rows < 0 or rows > 5 or (position ~= "top" and position ~= "bottom") then return nil end
  -- tmux suppresses status when the client cannot fit its configured rows.
  offset = position == "top" and tonumber(height) > rows and rows or 0
  return offset
end

function M.watch(callback)
  if type(callback) ~= "function" then return false end
  if watcher then watcher.callback = callback; return true end
  local pane = vim.env.TMUX_PANE
  if not loaded or not tmux.is_tmux or not pane or not pane:match("^%%%d+$") or not uv then return false end

  local base = vim.fn.tempname()
  local nonce = vim.fn.sha256(base .. ":" .. uv.os_getpid() .. ":" .. uv.hrtime()):sub(1, 32)
  local state = {
    nonce = nonce,
    dir = base .. "-" .. nonce,
  }
  state.signal = state.dir .. "/signal"
  state.script = state.dir .. "/cleanup"
  state.script_tmp = state.script .. ".tmp"
  if not uv.fs_mkdir(state.dir, 448) then return false end
  local signal_fd = uv.fs_open(state.signal, "w", 384)
  if not signal_fd then remove_files(state); return false end
  uv.fs_close(signal_fd)

  local args = { "tmux" }
  local command = signal_command(state.script)
  for i, event in ipairs(hooks) do
    if i > 1 then args[#args + 1] = ";" end
    args[#args + 1] = "set-hook"
    args[#args + 1] = "-ga"
    args[#args + 1] = event
    args[#args + 1] = command
  end
  tmux_call(args)

  local output = tmux_call({ "tmux", "show-hooks", "-g" })
  local indices, count = {}, 0
  if output then
    for _, entry in ipairs(owned_hooks(output, nonce)) do
      local event = entry[1]
      if vim.tbl_contains(hooks, event) and not indices[event] then
        indices[event], count = entry[2], count + 1
      end
    end
  end
  if count ~= #hooks then
    remove_hooks(nonce)
    remove_files(state)
    return false
  end

  if not write_script(state, cleanup_script(state, indices)) then
    remove_hooks(nonce)
    remove_files(state)
    return false
  end

  local made, fs_event = pcall(uv.new_fs_event)
  if not made or not fs_event then
    remove_hooks(nonce)
    remove_files(state)
    return false
  end
  state.watcher = fs_event
  local start_ok, started = pcall(state.watcher.start, state.watcher, state.signal, {}, function()
    if not state.active or watcher ~= state then return end
    offset, recipients, live_clients = nil, nil, nil
    if state.scheduled then return end
    state.scheduled = true
    vim.schedule(function()
      state.scheduled = false
      if state.active and watcher == state then state.callback() end
    end)
  end)
  if not start_ok or not started then
    remove_hooks(nonce)
    remove_files(state)
    return false
  end

  state.callback = callback
  state.active = true
  watcher = state
  if not leave_autocmd then
    local ok, id = pcall(vim.api.nvim_create_autocmd, "VimLeavePre", {
      once = true,
      callback = M.unwatch,
    })
    if not ok then M.unwatch(); return false end
    leave_autocmd = id
  end
  return true
end

function M.unwatch()
  local state = watcher
  if not state then return end
  watcher = nil
  recipients, live_clients = nil, nil
  state.active = false
  remove_hooks(state.nonce)
  remove_files(state)
end

return M
