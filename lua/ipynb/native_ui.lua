local M = {}

local position_ns = vim.api.nvim_create_namespace("ipynb-native-positions")
local output_ns = vim.api.nvim_create_namespace("ipynb-native-output")
local highlight_ns = vim.api.nvim_create_namespace("ipynb-native-highlight")
local buffers = {}

local defaults = {
  auto_open_output = false,
  cover_empty_lines = false,
  cover_lines_starting_with = {},
  enter_output_behavior = "open_and_enter",
  floating_window_focus = "top",
  image_location = "both",
  image_provider = "image.nvim",
  limit_output_chars = 1000000,
  open_cmd = nil,
  output_crop_border = true,
  output_show_exec_time = true,
  output_show_more = true,
  output_virt_lines = true,
  output_win_border = { "╭", "─", "╮", "│", "╯", "─", "╰", "│" },
  output_win_cover_gutter = false,
  output_win_hide_on_leave = false,
  output_win_max_height = 999999,
  output_win_max_width = 999999,
  output_win_style = nil,
  output_win_zindex = 50,
  split_direction = "right",
  split_size = 40,
  use_border_highlights = true,
  virt_lines_off_by_1 = true,
  virt_text_max_lines = 8,
  virt_text_output = true,
  virt_text_truncate = "bottom",
  wrap_output = true,
}

local function valid_buf(buf)
  return type(buf) == "number" and vim.api.nvim_buf_is_valid(buf)
end

local function valid_win(win)
  return type(win) == "number" and vim.api.nvim_win_is_valid(win)
end

local function number(value, fallback)
  value = tonumber(value)
  if value == nil or value ~= value then return fallback end
  return value
end

local function normalize_position(value)
  value = type(value) == "table" and value or {}
  return {
    line = math.max(0, math.floor(number(value.line or value.lineno or value[1], 0))),
    col = math.max(0, math.floor(number(value.col or value.colno or value[2], 0))),
  }
end

local function position_before(left, right)
  return left.line < right.line or (left.line == right.line and left.col < right.col)
end

local function position_at_or_before(left, right)
  return left.line < right.line or (left.line == right.line and left.col <= right.col)
end

local function notify(message, level)
  vim.schedule(function()
    vim.notify(message, level or vim.log.levels.WARN, { title = "ipynb" })
  end)
end

local function extmark_position(buf, id)
  if not valid_buf(buf) or not id then return nil end
  local ok, result = pcall(vim.api.nvim_buf_get_extmark_by_id, buf, position_ns, id, {})
  if not ok or type(result) ~= "table" or #result < 2 then return nil end
  return { line = result[1], col = result[2] }
end

local function set_position(buf, pos, id, right_gravity)
  local opts = { strict = false, right_gravity = right_gravity }
  if id then opts.id = id end
  return vim.api.nvim_buf_set_extmark(buf, position_ns, pos.line, pos.col, opts)
end

local function source_text(buf, begin_pos, end_pos)
  if not valid_buf(buf) or not position_before(begin_pos, end_pos) then return "" end
  local count = vim.api.nvim_buf_line_count(buf)
  if count == 0 or begin_pos.line >= count then return "" end
  local last_line = math.min(end_pos.line, count - 1)
  local lines = vim.api.nvim_buf_get_lines(buf, begin_pos.line, last_line + 1, false)
  if #lines == 0 then return "" end
  local first_col = math.min(begin_pos.col, #lines[1])
  local end_col = end_pos.col
  if end_pos.line >= count or end_col < 0 then end_col = #lines[#lines] end
  end_col = math.min(end_col, #lines[#lines])
  if #lines == 1 then return lines[1]:sub(first_col + 1, end_col) end
  lines[1] = lines[1]:sub(first_col + 1)
  lines[#lines] = lines[#lines]:sub(1, end_col)
  return table.concat(lines, "\n")
end

local function current_positions(cell)
  local begin_pos = extmark_position(cell.buf, cell.ui.begin_mark)
  local end_pos = extmark_position(cell.buf, cell.ui.end_mark)
  if not begin_pos or not end_pos then return nil end
  if cell.ui.end_fence then
    if end_pos.line <= begin_pos.line then return begin_pos, begin_pos end
    end_pos.line = end_pos.line - 1
    end_pos.col = #(vim.api.nvim_buf_get_lines(cell.buf, end_pos.line, end_pos.line + 1, false)[1] or "")
  end
  return begin_pos, end_pos
end

local function array_position(pos)
  return { pos.line, pos.col }
end

local function public_cell(cell)
  if not cell then return nil end
  local value = vim.deepcopy(cell.data)
  local begin_pos, end_pos = current_positions(cell)
  if begin_pos and end_pos then
    value.begin, value["end"] = array_position(begin_pos), array_position(end_pos)
    value.source = source_text(cell.buf, begin_pos, end_pos)
  end
  return value
end

local function find_cell(id, preferred_buf)
  if type(id) == "table" and id.ui then return id end
  if type(id) == "table" then id = id.id end
  preferred_buf = preferred_buf or vim.api.nvim_get_current_buf()
  local state = buffers[preferred_buf]
  if state and id ~= nil and state.by_id[tostring(id)] then return state.by_id[tostring(id)] end
  if state and id == nil then
    local current = M.current(preferred_buf)
    return current and state.by_id[tostring(current.id)] or nil
  end
  if id == nil then
    for _, candidate in pairs(buffers) do
      for _, cell in ipairs(candidate.order) do
        if cell.ui.float and cell.ui.float.buf == preferred_buf then return cell end
      end
    end
  end
  for _, candidate in pairs(buffers) do
    local cell = id ~= nil and candidate.by_id[tostring(id)] or nil
    if cell then return cell end
  end
end

local function source_window(buf)
  if vim.api.nvim_get_current_buf() == buf then return vim.api.nvim_get_current_win() end
  local wins = vim.fn.win_findbuf(buf)
  return wins and wins[1] or nil
end

local function closes_fence(line, opening)
  local closing = line:match("^(```+)%s*$")
  return opening ~= nil and closing ~= nil and #closing >= #opening
end

local function contains_cursor(buf, position, first, last)
  if first.line == last.line and first.col == last.col
    and position.line == first.line and position.col == first.col then return true end
  if position_at_or_before(first, position) and position_before(position, last) then return true end
  local row = position.line
  if row == first.line - 1 or row == last.line + 1 then
    local line = vim.api.nvim_buf_get_lines(buf, row, row + 1, false)[1] or ""
    return (row == first.line - 1 and line:match("^```+%s*[%w_+-]+") ~= nil)
      or (row == last.line + 1 and line:match("^```+%s*$") ~= nil)
  end
  return false
end

-- Own notebook state and configuration; private renderers share this table.
local view = {
  buffers = buffers,
  options = vim.deepcopy(defaults),
  config_revision = 0,
  output_ns = output_ns,
  valid_buf = valid_buf,
  valid_win = valid_win,
  number = number,
  notify = notify,
  find_cell = find_cell,
  current_positions = current_positions,
  source_window = source_window,
}
local rich = require("ipynb.native_ui.rich")(view)
local output = require("ipynb.native_ui.output")(view, rich)

local function destroy_cell(cell)
  output.close_float(cell, false)
  output.clear_virtual(cell)
  if valid_buf(cell.buf) then
    if cell.ui.begin_mark then pcall(vim.api.nvim_buf_del_extmark, cell.buf, position_ns, cell.ui.begin_mark) end
    if cell.ui.end_mark then pcall(vim.api.nvim_buf_del_extmark, cell.buf, position_ns, cell.ui.end_mark) end
  end
end

local function refresh(buf, force, cursor_only)
  if view.shutting_down then return end
  local state = buffers[buf]
  if not state or not valid_buf(buf) then return end
  state.layout_known_spans = nil
  pcall(vim.api.nvim_buf_clear_namespace, buf, highlight_ns, 0, -1)
  local changedtick = vim.api.nvim_buf_get_changedtick(buf)
  local positions
  if cursor_only and state.changedtick == changedtick then
    state.cursor_positions = state.cursor_positions or {}
    positions = state.cursor_positions
  else
    state.cursor_positions = nil
  end
  local selected = nil
  local win = source_window(buf)
  if valid_win(win) then
    local cursor = vim.api.nvim_win_get_cursor(win)
    local position = { line = cursor[1] - 1, col = cursor[2] }
    for _, cell in ipairs(state.order) do
      local cached = positions and positions[cell]
      local begin_pos, end_pos
      if cached then
        begin_pos, end_pos = cached[1], cached[2]
      else
        begin_pos, end_pos = current_positions(cell)
        if positions then positions[cell] = { begin_pos, end_pos } end
      end
      if begin_pos and end_pos and contains_cursor(buf, position, begin_pos, end_pos) then
        selected = cell
        break
      end
    end
  end
  local selected_id = selected and tostring(selected.id) or nil
  if state.selected_id ~= selected_id then
    local previous = state.selected_id and state.by_id[state.selected_id] or nil
    if previous and output.close_float(previous, false) then output.show_virtual(previous, false) end
    if not state.selected_id and selected and view.options.auto_open_output then output.render_float(selected, false) end
    state.selected_id = selected_id
  end
  if cursor_only and state.changedtick == changedtick then
    for _, cell in ipairs(state.order) do
      if cell.ui.float and valid_win(cell.ui.float.win) then output.render_float(cell, false) end
    end
    return
  end
  local context = {}
  for _, cell in ipairs(state.order) do
    output.show_virtual(cell, force, context)
    if cell.ui.float and valid_win(cell.ui.float.win) then output.render_float(cell, false) end
  end
  require("ipynb.cell_header").refresh(buf)
  state.changedtick = changedtick
end

function M.configure(opts)
  view.shutting_down = false
  opts = type(opts) == "table" and vim.deepcopy(opts) or {}
  if type(opts.output) == "table" then
    opts.output_win_max_height = opts.output_win_max_height or opts.output.height
    opts.output_win_max_width = opts.output_win_max_width or opts.output.width
    opts.virt_text_max_lines = opts.virt_text_max_lines or opts.output.preview_lines
    opts.output = nil
  end
  for key in pairs(defaults) do
    local global = vim.g["ipynb_" .. key]
    if global ~= nil then view.options[key] = global end
  end
  view.options = vim.tbl_deep_extend("force", view.options, opts)
  view.config_revision = view.config_revision + 1
  for buf in pairs(buffers) do refresh(buf, true) end
end

function M.sync(buf, cells)
  if view.shutting_down then return false end
  buf = number(buf, vim.api.nvim_get_current_buf())
  if not valid_buf(buf) or type(cells) ~= "table" then return false end
  local state = buffers[buf] or { by_id = {}, order = {}, selected_id = nil }
  buffers[buf] = state
  local seen, order, changed_any = {}, {}, false

  for _, incoming in ipairs(cells) do
    if type(incoming) == "table" and incoming.id ~= nil then
      local key = tostring(incoming.id)
      local cell = state.by_id[key]
      local normalized = vim.tbl_extend("force", incoming, { buf = buf })
      local changed = not cell or not vim.deep_equal(cell.data, normalized)
      local previous_status = cell and cell.data and cell.data.status or nil
      if not cell then
        local begin_pos = normalize_position(incoming.begin)
        local end_pos = normalize_position(incoming["end"])
        local opening = begin_pos.line > 0 and vim.api.nvim_buf_get_lines(buf, begin_pos.line - 1, begin_pos.line, false)[1] or ""
        local fence = opening:match("^(```+)%s*[%w_+-]+")
        local ending = vim.api.nvim_buf_get_lines(buf, end_pos.line, end_pos.line + 2, false)
        local empty = begin_pos.line == end_pos.line and begin_pos.col == 0 and end_pos.col == 0
          and closes_fence(ending[1] or "", fence)
        local end_fence = begin_pos.col == 0 and fence ~= nil
          and (empty or (end_pos.col == #(ending[1] or "") and closes_fence(ending[2] or "", fence)))
        -- Anchor whole cells to their closing fence so linewise paste after EOF expands them.
        if end_fence and not empty then end_pos = { line = end_pos.line + 1, col = 0 } end
        cell = {
          id = incoming.id,
          buf = buf,
          revision = 0,
          ui = {
            begin_mark = set_position(buf, begin_pos, nil, false),
            end_mark = set_position(buf, end_pos, nil, true),
            end_fence = end_fence,
            hidden = false,
            images = { virt = {}, float = {} },
          },
        }
        state.by_id[key] = cell
      end
      if changed then cell.data = vim.deepcopy(normalized) end
      if cell.ui.hidden and previous_status ~= normalized.status
        and (normalized.status == "hold" or normalized.status == "running") then
        cell.ui.hidden = false
      end
      if changed then
        changed_any = true
        cell.revision = cell.revision + 1
        cell.ui.virt_key, cell.ui.float_key = nil, nil
      end
      seen[key], order[#order + 1] = true, cell
    end
  end

  for id, cell in pairs(state.by_id) do
    if not seen[id] then
      destroy_cell(cell)
      state.by_id[id] = nil
      if state.selected_id == id then state.selected_id = nil end
    end
  end
  local positions = {}
  for _, cell in ipairs(order) do
    positions[cell] = extmark_position(buf, cell.ui.begin_mark) or normalize_position(cell.data.begin)
  end
  table.sort(order, function(left, right) return position_before(positions[left], positions[right]) end)
  state.order = order
  if #order == 0 then
    for _, win in ipairs(vim.fn.win_findbuf(buf)) do rich.image_canvas(win, false) end
  end
  refresh(buf, false)
  return true, changed_any
end

function M.positions(buf)
  buf = number(buf, vim.api.nvim_get_current_buf())
  local state = buffers[buf]
  if not state or not valid_buf(buf) then return {} end
  local result = {}
  for _, cell in ipairs(state.order) do
    local begin_pos, end_pos = current_positions(cell)
    if begin_pos and end_pos then
      result[#result + 1] = {
        id = cell.id,
        begin = array_position(begin_pos),
        ["end"] = array_position(end_pos),
        source = source_text(buf, begin_pos, end_pos),
      }
    end
  end
  return result
end

function M.with_positions(buf, callback)
  local before = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local marks = vim.api.nvim_buf_get_extmarks(buf, position_ns, 0, -1, { details = true })
  local ok, err = xpcall(callback, debug.traceback)
  if valid_buf(buf) and #marks > 0 then
    local after = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    -- Reapply save-hook changes as a minimal diff so whole-buffer formatters
    -- cannot collapse every output anchor to the beginning of the notebook.
    local shadow = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_lines(shadow, 0, -1, false, before)
    for _, mark in ipairs(marks) do
      set_position(shadow, { line = mark[2], col = mark[3] }, mark[1], mark[4].right_gravity)
    end
    local changes = vim.diff(table.concat(before, "\n") .. "\n", table.concat(after, "\n") .. "\n",
      { result_type = "indices", algorithm = "histogram" })
    for index = #changes, 1, -1 do
      local change = changes[index]
      local start = change[1] - (change[2] > 0 and 1 or 0)
      local replacement = vim.list_slice(after, change[3], change[3] + change[4] - 1)
      vim.api.nvim_buf_set_lines(shadow, start, start + change[2], false, replacement)
    end
    for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(shadow, position_ns, 0, -1, { details = true })) do
      set_position(buf, { line = mark[2], col = mark[3] }, mark[1], mark[4].right_gravity)
    end
    vim.api.nvim_buf_delete(shadow, { force = true })
  end
  if not ok then error(err) end
end

function M.cells(buf)
  buf = number(buf, vim.api.nvim_get_current_buf())
  local state = buffers[buf]
  if not state then return {} end
  local positions_by_id = {}
  for _, item in ipairs(M.positions(buf)) do positions_by_id[item.id] = item end
  local result = {}
  for _, cell in ipairs(state.order) do
    local value = vim.deepcopy(cell.data)
    local position = positions_by_id[cell.id]
    if position then
      value.begin, value["end"], value.source = position.begin, position["end"], position.source
    end
    result[#result + 1] = value
  end
  return result
end

function M.cell_headers(buf)
  local result, by_row = {}, {}
  for _, cell in ipairs(buffers[buf] and buffers[buf].order or {}) do
    local first = current_positions(cell)
    if first then
      result[#result + 1] = { row = first.line - 1, execution_count = cell.data.execution_count,
        status = cell.data.status, success = cell.data.success, old = cell.data.old, group = rich.border_group(cell) }
      by_row[first.line - 1] = result[#result]
    end
  end
  if vim.b[buf].ipynb_code_spans_tick == vim.api.nvim_buf_get_changedtick(buf) then
    result = {}
    for _, span in ipairs(vim.b[buf].ipynb_code_spans) do
      local row = span.begin[1] - 1
      result[#result + 1] = by_row[row] or { row = row, status = "new", group = "IpyNotebookCodeLabel" }
    end
  end
  return result
end

function M.current(buf, row, col)
  buf = number(buf, vim.api.nvim_get_current_buf())
  local state = buffers[buf]
  if not state then return nil end
  if row == nil then
    local win = source_window(buf)
    if not valid_win(win) then return nil end
    local cursor = vim.api.nvim_win_get_cursor(win)
    row, col = cursor[1] - 1, cursor[2]
  else
    row, col = number(row, 0), number(col, 0)
  end
  local position = { line = row, col = col }
  for _, cell in ipairs(state.order) do
    local begin_pos, end_pos = current_positions(cell)
    if begin_pos and end_pos and contains_cursor(buf, position, begin_pos, end_pos) then
      return public_cell(cell)
    end
  end
end

function M.clear(buf)
  buf = number(buf, vim.api.nvim_get_current_buf())
  local state = buffers[buf]
  if not state then return false end
  require("ipynb.cell_header").clear(buf)
  for _, cell in ipairs(state.order) do destroy_cell(cell) end
  if valid_buf(buf) then
    pcall(vim.api.nvim_buf_clear_namespace, buf, position_ns, 0, -1)
    pcall(vim.api.nvim_buf_clear_namespace, buf, output_ns, 0, -1)
    pcall(vim.api.nvim_buf_clear_namespace, buf, highlight_ns, 0, -1)
  end
  for _, win in ipairs(vim.fn.win_findbuf(buf)) do rich.image_canvas(win, false) end
  buffers[buf] = nil
  return true
end

function M.shutdown()
  if view.shutting_down then return end
  rich.shutdown()
  for buf in pairs(buffers) do M.clear(buf) end
end

function M.refresh(buf, cursor_only)
  if view.shutting_down then return end
  buf = number(buf, vim.api.nvim_get_current_buf())
  if buffers[buf] then
    refresh(buf, false, cursor_only)
    return
  end
  for _, state in pairs(buffers) do
    for _, cell in ipairs(state.order) do
      if cell.ui.float and cell.ui.float.buf == buf and valid_win(cell.ui.float.win) then
        output.render_float(cell, false)
        return
      end
    end
  end
end

M.open = output.open
M.show = output.show
M.hide = output.hide
M.toggle = output.toggle
M.image_popup = rich.image_popup
M.open_browser = rich.open_browser
M.yank = rich.yank

return M
