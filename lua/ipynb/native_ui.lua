local M = {}

local position_ns = vim.api.nvim_create_namespace("ipynb-native-positions")
local output_ns = vim.api.nvim_create_namespace("ipynb-native-output")
local highlight_ns = vim.api.nvim_create_namespace("ipynb-native-highlight")

local buffers = {}
local config_revision = 0
local saved_fillchars = {}

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
  output_win_cover_gutter = true,
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

local options = vim.deepcopy(defaults)

local function valid_buf(buf)
  return type(buf) == "number" and vim.api.nvim_buf_is_valid(buf)
end

local function valid_win(win)
  return type(win) == "number" and vim.api.nvim_win_is_valid(win)
end

local function image_canvas(win, enabled)
  if not valid_win(win) then
    if type(win) == "number" then saved_fillchars[win] = nil end
    return
  end
  if enabled then
    if saved_fillchars[win] == nil then
      saved_fillchars[win] = vim.api.nvim_get_option_value("fillchars", { scope = "local", win = win })
    end
    vim.api.nvim_win_call(win, function() vim.opt_local.fillchars:append({ eob = " " }) end)
  elseif saved_fillchars[win] ~= nil then
    pcall(vim.api.nvim_set_option_value, "fillchars", saved_fillchars[win], { scope = "local", win = win })
    saved_fillchars[win] = nil
  end
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

local function safe_id(value)
  return tostring(value):gsub("[^%w_.-]", "_")
end

local function notify(message, level)
  vim.schedule(function()
    vim.notify(message, level or vim.log.levels.WARN, { title = "ipynb" })
  end)
end

local none_api = {
  from_file = function() return nil end,
  image_size = function() return { width = 0, height = 0 } end,
  render = function() end,
  clear = function() end,
  refresh = function() end,
  layout_key = function() return {} end,
}

local providers = { none = none_api }

local function image_nvim_api()
  if not providers["image.nvim"] then
    local ok, module = pcall(require, "ipynb.molten_load_image_nvim")
    providers["image.nvim"] = ok and module.image_api or none_api
  end
  return providers["image.nvim"]
end

local function snacks_api()
  if not providers["snacks.nvim"] then
    local ok, module = pcall(require, "ipynb.molten_load_snacks_nvim")
    local api = ok and module and module.snacks_api or nil
    providers["snacks.nvim"] = api and {
      from_file = api.from_file,
      image_size = api.image_size,
      render = api.render,
      clear = api.clear,
      refresh = function() end,
      layout_key = function() return {} end,
    } or none_api
  end
  return providers["snacks.nvim"]
end

local function wezterm_api()
  if providers.wezterm then return providers.wezterm end
  local ok, module = pcall(require, "ipynb.molten_load_wezterm_nvim")
  local bridge = ok and module and module.wezterm_api or nil
  if not bridge then
    providers.wezterm = none_api
    return providers.wezterm
  end

  local images = {}
  local initial_pane
  local image_pane
  providers.wezterm = {
    external = true,
    from_file = function(path, opts)
      local id = opts.id or path
      images[id] = path
      return id
    end,
    image_size = function() return { width = 0, height = 0 } end,
    layout_key = function() return {} end,
    clear = function(id) images[id] = nil end,
    refresh = function() end,
    render = function(id)
      local path = images[id]
      if not path then return end
      if not initial_pane then initial_pane = bridge.get_pane_id() end
      if not image_pane then
        image_pane = bridge.wezterm_ipynb_init(initial_pane, options.split_direction, options.split_size)
      end
      bridge.send_image(path, tostring(image_pane), tostring(initial_pane))
    end,
  }
  return providers.wezterm
end

local function provider()
  if options.image_provider == "image.nvim" then return image_nvim_api() end
  if options.image_provider == "snacks.nvim" then return snacks_api() end
  if options.image_provider == "wezterm" then return wezterm_api() end
  return none_api
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

local function state_for(buf)
  return buffers[buf]
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

local function image_records(cell, kind)
  cell.ui.images[kind] = cell.ui.images[kind] or {}
  return cell.ui.images[kind]
end

local function clear_images(cell, kind)
  local records = image_records(cell, kind)
  for _, record in ipairs(records) do pcall(record.api.clear, record.id) end
  cell.ui.images[kind] = {}
  local api = provider()
  if api.refresh then pcall(api.refresh) end
end

local function clear_virtual(cell)
  clear_images(cell, "virt")
  if cell.ui.virt_mark and valid_buf(cell.buf) then
    pcall(vim.api.nvim_buf_del_extmark, cell.buf, output_ns, cell.ui.virt_mark)
  end
  cell.ui.virt_mark = nil
  cell.ui.virt_key = nil
end

local function close_float(cell, restore_source)
  clear_images(cell, "float")
  local float = cell.ui.float
  if not float then return false end
  local was_current = valid_win(float.win) and vim.api.nvim_get_current_win() == float.win
  if valid_win(float.win) then pcall(vim.api.nvim_win_close, float.win, true) end
  if valid_buf(float.buf) then pcall(vim.api.nvim_buf_delete, float.buf, { force = true }) end
  if cell.ui.reserve_mark and valid_buf(cell.buf) then
    pcall(vim.api.nvim_buf_del_extmark, cell.buf, output_ns, cell.ui.reserve_mark)
  end
  cell.ui.reserve_mark = nil
  cell.ui.float = nil
  cell.ui.float_key = nil
  if restore_source and was_current and valid_win(float.source_win) then
    pcall(vim.api.nvim_set_current_win, float.source_win)
  end
  return true
end

local function destroy_cell(cell)
  close_float(cell, false)
  clear_virtual(cell)
  if valid_buf(cell.buf) then
    if cell.ui.begin_mark then pcall(vim.api.nvim_buf_del_extmark, cell.buf, position_ns, cell.ui.begin_mark) end
    if cell.ui.end_mark then pcall(vim.api.nvim_buf_del_extmark, cell.buf, position_ns, cell.ui.end_mark) end
  end
end

local function border_size(border)
  if type(border) == "string" then
    if border == "rounded" or border == "single" or border == "double" or border == "solid" then return 2, 2 end
    if border == "shadow" then return 1, 1 end
    return 0, 0
  end
  if type(border) ~= "table" or #border == 0 then return 0, 0 end
  local function present(index)
    local item = border[((index - 1) % #border) + 1]
    if type(item) == "table" then item = item[1] end
    return type(item) == "string" and vim.fn.strdisplaywidth(item) > 0 and 1 or 0
  end
  return present(8) + present(4), present(2) + present(6)
end

local function border_group(cell)
  local group = "IpynbOutputBorder"
  if cell.data.status == "hold" then
    group = "IpynbOutputBorderQueued"
  elseif cell.data.status == "running" then
    group = "IpynbOutputBorderRunning"
  elseif cell.data.status == "done" and cell.data.success == false then
    group = "IpynbOutputBorderFail"
  elseif cell.data.status == "done" then
    group = "IpynbOutputBorderSuccess"
  end
  return group
end

local function border_with_highlight(border, cell)
  if not options.use_border_highlights or type(border) ~= "table" then return border end
  local group = border_group(cell)
  border = vim.deepcopy(border)
  for index, value in ipairs(border) do
    border[index] = type(value) == "table" and { value[1], group } or { value, group }
  end
  return border
end

local function header(cell)
  local status = cell.data.status or "new"
  local count = cell.data.execution_count
  local execution = (count == nil or count == vim.NIL) and "..." or tostring(count)
  local label
  if status == "hold" then
    label = "* On Hold"
  elseif status == "running" then
    label = "... Running"
  elseif status == "done" then
    label = cell.data.success == false and "✗ Failed" or "✓ Done"
  else
    return "╭─ Out[_]: Never Run"
  end
  local old = cell.data.old and "[OLD] " or ""
  local elapsed = ""
  if not cell.data.old and options.output_show_exec_time and cell.data.elapsed ~= nil then
    local value = tonumber(cell.data.elapsed)
    elapsed = value and string.format(" %.2fs", value) or (" " .. tostring(cell.data.elapsed))
  end
  return ("╭─ %sOut[%s]: %s%s"):format(old, execution, label, elapsed)
end

local function replay_controls(text)
  if not text:find("[\r\b]") then return text:gsub("\r\n", "\n") end
  local rows, row, col = { {} }, 1, 1
  text = text:gsub("\r\n", "\n")
  for char in text:gmatch("[\0-\127\194-\244][\128-\191]*") do
    if char == "\n" then
      rows[#rows + 1], row, col = {}, row + 1, 1
    elseif char == "\r" then
      col = 1
    elseif char == "\b" then
      col = math.max(1, col - 1)
    else
      rows[row][col], col = char, col + 1
    end
  end
  local result = {}
  for _, chars in ipairs(rows) do result[#result + 1] = table.concat(chars) end
  return table.concat(result, "\n")
end

local function take_display_width(text, width)
  if width < 1 or text == "" then return "", text end
  local chars = vim.fn.strchars(text)
  local low, high, best = 1, chars, 0
  while low <= high do
    local middle = math.floor((low + high) / 2)
    local part = vim.fn.strcharpart(text, 0, middle)
    if vim.fn.strdisplaywidth(part) <= width then
      best, low = middle, middle + 1
    else
      high = middle - 1
    end
  end
  if best == 0 then best = 1 end
  return vim.fn.strcharpart(text, 0, best), vim.fn.strcharpart(text, best)
end

local function wrap_lines(lines, width)
  if not options.wrap_output or width < 1 then return lines end
  local result = {}
  for _, value in ipairs(lines) do
    local line = tostring(value)
    if line == "" then
      result[#result + 1] = ""
    else
      while vim.fn.strdisplaywidth(line) > width do
        local part
        part, line = take_display_width(line, width)
        result[#result + 1] = part
      end
      result[#result + 1] = line
    end
  end
  return result
end

local function truncate_lines(lines)
  local maximum = math.max(1, number(options.virt_text_max_lines, 8) - 2)
  if #lines <= maximum then return lines end
  if options.virt_text_truncate == "top" and maximum > 2 then
    local result = { lines[1], ("↑ %d More lines"):format(#lines - maximum) }
    for index = #lines - maximum + 3, #lines do result[#result + 1] = lines[index] end
    return result
  end
  local result = {}
  for index = 1, maximum - 1 do result[#result + 1] = lines[index] end
  result[#result + 1] = ("󰁅 %d More lines "):format(#lines - maximum + 1)
  return result
end

local function text_lines(segment)
  local value = segment.lines or segment.text or {}
  if type(value) == "table" then
    local result = {}
    for _, line in ipairs(value) do result[#result + 1] = tostring(line) end
    return result
  end
  return vim.split(replay_controls(tostring(value)), "\n", { plain = true })
end

local function segments(cell)
  if type(cell.data.segments) == "table" and #cell.data.segments > 0 then return cell.data.segments end
  local result = {}
  if type(cell.data.lines) == "table" then result[#result + 1] = { kind = "text", lines = cell.data.lines } end
  if type(cell.data.images) == "table" then
    for _, image in ipairs(cell.data.images) do
      result[#result + 1] = { kind = "image", path = image.path, mimetype = image.mimetype }
    end
  end
  return result
end

local function pad(line, width)
  line = tostring(line)
  local display = vim.fn.strdisplaywidth(line)
  if display > width then
    line = take_display_width(line, width)
    display = vim.fn.strdisplaywidth(line)
  end
  return line .. string.rep(" ", math.max(0, width - display))
end

local function render_body(cell, shape, target_buf, target_win, virtual)
  local kind = virtual and "virt" or "float"
  clear_images(cell, kind)
  local api = provider()
  local records = image_records(cell, kind)
  local body = {}
  local remaining = number(options.limit_output_chars, 1000000)
  local image_index = 0

  for _, segment in ipairs(segments(cell)) do
    if segment.kind == "image" then
      local location = options.image_location
      local allowed = location == "both" or (virtual and location == "virt") or (not virtual and location == "float")
      if allowed and type(segment.path) == "string" and segment.path ~= "" then
        image_index = image_index + 1
        local id = ("ipynb-native-%d-%s-%d-%d-%s"):format(
          cell.buf, safe_id(cell.id), cell.revision, image_index, kind
        )
        local row = #body + 1
        local ok, identifier = pcall(api.from_file, segment.path, {
          id = id,
          buffer = target_buf,
          window = target_win,
          x = 0,
          y = virtual and shape.row or row,
          render_offset_top = virtual and row or 0,
          with_virtual_padding = false,
          max_width = shape.width,
          max_height = math.max(0, shape.height - 2),
        })
        local size_ok, size = false, nil
        if ok and identifier then size_ok, size = pcall(api.image_size, identifier) end
        local height = size_ok and type(size) == "table" and number(size.height, 0) or 0
        if identifier and api.external then
          records[#records + 1] = { api = api, id = identifier }
          pcall(api.render, identifier)
          body[#body + 1] = segment.fallback or "[Image displayed in external pane]"
        elseif identifier and height > 0 then
          records[#records + 1] = { api = api, id = identifier }
          for _ = 1, height do body[#body + 1] = " " end
        else
          if identifier then pcall(api.clear, identifier) end
          body[#body + 1] = segment.fallback or "[Image unavailable: no usable image renderer]"
        end
      end
    else
      local lines = text_lines(segment)
      local joined = replay_controls(table.concat(lines, "\n"))
      if options.limit_output_chars and options.limit_output_chars > 0 then
        if #joined > remaining then
          joined = joined:sub(1, remaining) .. ("\n...truncated to %d chars"):format(options.limit_output_chars)
        end
        remaining = math.max(0, remaining - #joined)
      end
      lines = wrap_lines(vim.split(joined, "\n", { plain = true }), shape.width)
      if virtual then lines = truncate_lines(lines) end
      vim.list_extend(body, lines)
    end
  end

  local title = header(cell)
  if vim.fn.strdisplaywidth(title) > shape.width then title = take_display_width(title, shape.width) end
  local lines = { title .. string.rep("─", math.max(0, shape.width - vim.fn.strdisplaywidth(title))) }
  for _, line in ipairs(body) do lines[#lines + 1] = pad(line, shape.width) end
  lines[#lines + 1] = "╰" .. string.rep("─", math.max(0, shape.width - 1))
  return lines, records
end

local function source_window(buf)
  if vim.api.nvim_get_current_buf() == buf then return vim.api.nvim_get_current_win() end
  local wins = vim.fn.win_findbuf(buf)
  return wins and wins[1] or nil
end

local function cover_offset(buf, anchor)
  if not options.cover_empty_lines then return 0 end
  local offset, line = 0, anchor.line
  while line > 0 do
    local text = vim.api.nvim_buf_get_lines(buf, line, line + 1, false)[1] or ""
    local covered = text == ""
    for _, prefix in ipairs(options.cover_lines_starting_with or {}) do
      if vim.startswith(text, prefix) then covered = true break end
    end
    if not covered then return offset end
    line, offset = line - 1, offset - 1
  end
  return 0
end

local function show_virtual(cell, force)
  if not options.virt_text_output or cell.ui.hidden or (cell.ui.float and valid_win(cell.ui.float.win)) then
    clear_virtual(cell)
    return
  end
  local begin_pos, end_pos = current_positions(cell)
  if not begin_pos or not end_pos then return end
  local win = source_window(cell.buf)
  if not valid_win(win) then return end
  local info = vim.fn.getwininfo(win)[1]
  if not info then return end
  local width = info.width - info.textoff
  if width < 1 then clear_virtual(cell) return end
  local anchor = { line = end_pos.line + cover_offset(cell.buf, end_pos), col = 0 }
  local count = vim.api.nvim_buf_line_count(cell.buf)
  if options.virt_lines_off_by_1 and anchor.line < count - 1 then anchor.line = anchor.line + 1 end
  anchor.line = math.max(0, math.min(anchor.line, math.max(0, count - 1)))
  local api = provider()
  local layout = api.layout_key and api.layout_key() or {}
  local key = table.concat({ cell.revision, config_revision, win, anchor.line, width, info.height, vim.inspect(layout) }, ":")
  if not force and cell.ui.virt_mark and cell.ui.virt_key == key then return end
  local lines = render_body(cell, {
    row = anchor.line,
    width = width,
    height = info.height,
  }, cell.buf, win, true)
  local virt_lines = {}
  for index, line in ipairs(lines) do
    local group = (index == 1 or index == #lines) and border_group(cell) or "IpynbVirtualText"
    virt_lines[#virt_lines + 1] = { { line, group } }
  end
  local mark_options = { virt_lines = virt_lines, strict = false }
  if cell.ui.virt_mark then mark_options.id = cell.ui.virt_mark end
  cell.ui.virt_mark = vim.api.nvim_buf_set_extmark(cell.buf, output_ns, anchor.line, 0, mark_options)
  cell.ui.virt_key = key
  for _, record in ipairs(image_records(cell, "virt")) do pcall(record.api.render, record.id) end
  if #image_records(cell, "virt") > 0 then image_canvas(win, true) end
  if api.refresh then pcall(api.refresh) end
end

local function source_relative_row(win, row)
  local info = vim.fn.getwininfo(win)[1]
  if not info then return 0 end
  local count = vim.api.nvim_buf_line_count(vim.api.nvim_win_get_buf(win))
  local pos = vim.fn.screenpos(win, math.min(count, row + 1), 1)
  if not pos or pos.row == 0 then return 0 end
  return math.max(0, pos.row - info.winrow)
end

local function set_float_options(win)
  local values = {
    wrap = false,
    number = false,
    relativenumber = false,
    list = false,
    signcolumn = "no",
    foldcolumn = "0",
    foldenable = false,
    winbar = "",
    colorcolumn = "",
    statuscolumn = "",
    winblend = 0,
    cursorline = false,
    winhighlight = "Normal:IpynbOutputWin,NormalNC:IpynbOutputWinNC",
  }
  for name, value in pairs(values) do
    pcall(vim.api.nvim_set_option_value, name, value, { scope = "local", win = win })
  end
end

local function reserve_float_space(cell, height)
  if not options.output_virt_lines and not options.cover_empty_lines then return end
  local _, end_pos = current_positions(cell)
  if not end_pos then return end
  local row = end_pos.line + cover_offset(cell.buf, end_pos)
  if options.virt_lines_off_by_1 then row, height = row + 1, height - 1 end
  row = math.max(0, math.min(row, math.max(0, vim.api.nvim_buf_line_count(cell.buf) - 1)))
  local virtual = {}
  for _ = 1, math.max(0, height) do virtual[#virtual + 1] = { { "", "Normal" } } end
  local mark_options = { virt_lines = virtual, strict = false }
  if cell.ui.reserve_mark then mark_options.id = cell.ui.reserve_mark end
  cell.ui.reserve_mark = vim.api.nvim_buf_set_extmark(cell.buf, output_ns, row, 0, mark_options)
end

local function render_float(cell, focus)
  local _, end_pos = current_positions(cell)
  if not end_pos then return false end
  local float = cell.ui.float
  local source = float and float.source_win or source_window(cell.buf)
  if not valid_win(source) or vim.api.nvim_win_get_buf(source) ~= cell.buf then return false end
  local info = vim.fn.getwininfo(source)[1]
  if not info then return false end

  local border = border_with_highlight(vim.deepcopy(options.output_win_border), cell)
  local border_width, border_height = border_size(border)
  local screen_height = info.height - border_height
  local available_height = math.min(screen_height, number(options.output_win_max_height, 999999))
  local available_width = math.min(info.width - border_width, number(options.output_win_max_width, 999999))
  local gutter = options.output_win_cover_gutter and 0 or info.textoff
  available_width = available_width - gutter
  if available_width < 1 or available_height < 1 then return false end

  local float_key = table.concat({
    cell.revision, config_revision, source, end_pos.line, end_pos.col,
    info.width, info.height, info.topline,
  }, ":")
  if float and valid_win(float.win) and cell.ui.float_key == float_key then
    if focus then vim.cmd(("noautocmd call nvim_set_current_win(%d)"):format(float.win)) end
    return true
  end

  local display_buf = float and float.buf
  local previous_cursor
  local was_at_bottom = false
  if float and valid_win(float.win) and valid_buf(display_buf) then
    previous_cursor = vim.api.nvim_win_get_cursor(float.win)
    was_at_bottom = previous_cursor[1] == vim.api.nvim_buf_line_count(display_buf)
  end
  if not valid_buf(display_buf) then
    display_buf = vim.api.nvim_create_buf(false, true)
    vim.bo[display_buf].bufhidden = "wipe"
    vim.bo[display_buf].filetype = "ipynb_output"
  end

  clear_virtual(cell)
  local lines = render_body(cell, {
    row = 0,
    width = available_width,
    height = available_height,
  }, display_buf, float and float.win or nil, false)
  vim.api.nvim_buf_set_lines(display_buf, 0, -1, false, lines)

  local height = math.min(available_height, #lines)
  local row = source_relative_row(source, end_pos.line)
  row = math.max(0, math.min(row, info.height - border_height - height))
  local cropped = height == screen_height and #lines > height
  if cropped and options.output_crop_border and type(border) == "table" and #border > 0 then
    local expanded = {}
    for index = 1, 8 do expanded[index] = vim.deepcopy(border[((index - 1) % #border) + 1]) end
    local value = expanded[6]
    expanded[6] = type(value) == "table" and { "", value[2] } or ""
    border = expanded
  end

  local win_config = {
    relative = "win",
    win = source,
    row = row,
    col = gutter,
    width = available_width,
    height = height,
    border = border,
    focusable = true,
    zindex = number(options.output_win_zindex, 50),
  }
  if options.output_win_style then win_config.style = options.output_win_style end
  if options.output_show_more and not cropped and #lines > height then
    win_config.footer = { { (" 󰁅 %d More Lines "):format(#lines - height), "IpynbOutputFooter" } }
    win_config.footer_pos = "left"
  end

  if float and valid_win(float.win) then
    vim.api.nvim_win_set_config(float.win, win_config)
  else
    local ok, win = pcall(vim.api.nvim_open_win, display_buf, false, win_config)
    if not ok then
      pcall(vim.api.nvim_buf_delete, display_buf, { force = true })
      notify("Unable to display notebook output")
      return false
    end
    float = { buf = display_buf, win = win, source_win = source }
    cell.ui.float = float
    set_float_options(win)
    local hide = function() M.hide(false, cell.id) end
    vim.keymap.set("n", "q", hide, { buffer = display_buf, silent = true, nowait = true })
    vim.keymap.set("n", "<Esc>", hide, { buffer = display_buf, silent = true, nowait = true })
  end

  if previous_cursor then
    local last = vim.api.nvim_buf_line_count(display_buf)
    local target = was_at_bottom and last or math.min(previous_cursor[1], last)
    pcall(vim.api.nvim_win_set_cursor, float.win, { math.max(1, target), 0 })
  elseif options.floating_window_focus == "bottom" then
    pcall(vim.api.nvim_win_set_cursor, float.win, { vim.api.nvim_buf_line_count(display_buf), 0 })
  else
    pcall(vim.api.nvim_win_set_cursor, float.win, { 1, 0 })
  end

  reserve_float_space(cell, #lines + border_height)
  for _, record in ipairs(image_records(cell, "float")) do pcall(record.api.render, record.id) end
  local api = provider()
  if api.refresh then pcall(api.refresh) end
  cell.ui.float_key = float_key
  if focus and valid_win(float.win) then
    vim.cmd(("noautocmd call nvim_set_current_win(%d)"):format(float.win))
  end
  return true
end

local function highlight(cell)
  local begin_pos, end_pos = current_positions(cell)
  if not begin_pos or not end_pos or not position_before(begin_pos, end_pos) then return end
  pcall(vim.api.nvim_buf_set_extmark, cell.buf, highlight_ns, begin_pos.line, begin_pos.col, {
    end_row = end_pos.line,
    end_col = end_pos.col,
    hl_group = "IpynbCell",
    hl_eol = true,
    strict = false,
  })
end

local function refresh(buf, force)
  local state = state_for(buf)
  if not state or not valid_buf(buf) then return end
  pcall(vim.api.nvim_buf_clear_namespace, buf, highlight_ns, 0, -1)
  local selected = nil
  local win = source_window(buf)
  if valid_win(win) then
    local cursor = vim.api.nvim_win_get_cursor(win)
    local position = { line = cursor[1] - 1, col = cursor[2] }
    for _, cell in ipairs(state.order) do
      local begin_pos, end_pos = current_positions(cell)
      if begin_pos and end_pos and position_at_or_before(begin_pos, position)
        and position_before(position, end_pos) then
        selected = cell
        break
      end
    end
  end
  local selected_id = selected and tostring(selected.id) or nil
  if state.selected_id ~= selected_id then
    local previous = state.selected_id and state.by_id[state.selected_id] or nil
    if previous then close_float(previous, false) end
    if not state.selected_id and selected and options.auto_open_output then render_float(selected, false) end
    state.selected_id = selected_id
  end
  if selected then highlight(selected) end
  for _, cell in ipairs(state.order) do
    show_virtual(cell, force)
    if cell.ui.float and valid_win(cell.ui.float.win) then render_float(cell, false) end
  end
end

function M.configure(opts)
  opts = type(opts) == "table" and vim.deepcopy(opts) or {}
  if type(opts.output) == "table" then
    opts.output_win_max_height = opts.output_win_max_height or opts.output.height
    opts.output_win_max_width = opts.output_win_max_width or opts.output.width
    opts.virt_text_max_lines = opts.virt_text_max_lines or opts.output.preview_lines
    opts.output = nil
  end
  for key in pairs(defaults) do
    local global = vim.g["ipynb_" .. key]
    if global ~= nil then options[key] = global end
  end
  options = vim.tbl_deep_extend("force", options, opts)
  config_revision = config_revision + 1
  for buf in pairs(buffers) do refresh(buf, true) end
end

function M.sync(buf, cells)
  buf = number(buf, vim.api.nvim_get_current_buf())
  if not valid_buf(buf) or type(cells) ~= "table" then return false end
  local state = buffers[buf] or { by_id = {}, order = {}, selected_id = nil }
  buffers[buf] = state
  local seen, order = {}, {}

  for _, incoming in ipairs(cells) do
    if type(incoming) == "table" and incoming.id ~= nil then
      local key = tostring(incoming.id)
      local cell = state.by_id[key]
      local normalized = vim.deepcopy(incoming)
      normalized.id, normalized.buf = incoming.id, buf
      local changed = not cell or not vim.deep_equal(cell.data, normalized)
      local previous_status = cell and cell.data and cell.data.status or nil
      if not cell then
        local begin_pos = normalize_position(incoming.begin)
        local end_pos = normalize_position(incoming["end"])
        cell = {
          id = incoming.id,
          buf = buf,
          revision = 0,
          ui = {
            begin_mark = set_position(buf, begin_pos, nil, false),
            end_mark = set_position(buf, end_pos, nil, true),
            hidden = false,
            images = { virt = {}, float = {} },
          },
        }
        state.by_id[key] = cell
      end
      cell.data = normalized
      if cell.ui.hidden and previous_status ~= normalized.status
        and (normalized.status == "hold" or normalized.status == "running") then
        cell.ui.hidden = false
      end
      if changed then
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
  table.sort(order, function(left, right)
    local left_pos = extmark_position(buf, left.ui.begin_mark) or normalize_position(left.data.begin)
    local right_pos = extmark_position(buf, right.ui.begin_mark) or normalize_position(right.data.begin)
    return position_before(left_pos, right_pos)
  end)
  state.order = order
  if #order == 0 then
    for _, win in ipairs(vim.fn.win_findbuf(buf)) do image_canvas(win, false) end
  end
  refresh(buf, false)
  return true
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
    if begin_pos and end_pos and position_at_or_before(begin_pos, position) and position_before(position, end_pos) then
      return public_cell(cell)
    end
  end
end

function M.open(cell_id)
  local cell = find_cell(cell_id)
  if not cell then return false end
  local behavior = options.enter_output_behavior
  local existed = cell.ui.float and valid_win(cell.ui.float.win)
  if not existed and behavior == "no_open" then return false end
  cell.ui.hidden = false
  local focus = existed or behavior == "open_and_enter"
  return render_float(cell, focus)
end

function M.show(cell_id)
  local cell = find_cell(cell_id)
  if not cell then return false end
  cell.ui.hidden = false
  return render_float(cell, false)
end

function M.hide(explicit, cell_id)
  local hidden = false
  local target = explicit and find_cell(cell_id) or nil
  if target then
    target.ui.hidden = true
    clear_virtual(target)
    hidden = true
  end
  for _, state in pairs(buffers) do
    for _, cell in ipairs(state.order) do
      if cell.ui.float and valid_win(cell.ui.float.win) then
        hidden = close_float(cell, true) or hidden
        if not cell.ui.hidden then show_virtual(cell, true) end
      end
    end
  end
  return hidden
end

function M.clear(buf)
  buf = number(buf, vim.api.nvim_get_current_buf())
  local state = buffers[buf]
  if not state then return false end
  for _, cell in ipairs(state.order) do destroy_cell(cell) end
  if valid_buf(buf) then
    pcall(vim.api.nvim_buf_clear_namespace, buf, position_ns, 0, -1)
    pcall(vim.api.nvim_buf_clear_namespace, buf, output_ns, 0, -1)
    pcall(vim.api.nvim_buf_clear_namespace, buf, highlight_ns, 0, -1)
  end
  for _, win in ipairs(vim.fn.win_findbuf(buf)) do image_canvas(win, false) end
  buffers[buf] = nil
  return true
end

local function open_path(path, command)
  if type(path) ~= "string" or path == "" then return false end
  if type(command) == "string" and command ~= "" then
    vim.system({ command, path }, { detach = true }, function(result)
      if result.code ~= 0 then notify("Could not open output: " .. (result.stderr or "unknown error")) end
    end)
    return true
  end
  if vim.ui.open then
    local ok = pcall(vim.ui.open, path)
    return ok
  end
  local executable = vim.fn.has("mac") == 1 and "open" or (vim.fn.has("win32") == 1 and "start" or "xdg-open")
  vim.system({ executable, path }, { detach = true })
  return true
end

function M.image_popup(cell_id)
  local cell = find_cell(cell_id)
  if not cell then return false end
  local path
  for _, segment in ipairs(segments(cell)) do
    if segment.kind == "image" and type(segment.path) == "string" and segment.path ~= "" then
      path = segment.path
      break
    end
  end
  if not path then return false end
  if options.image_provider == "image.nvim" then
    return require("ipynb.image_viewer").open(path)
  end
  return open_path(path)
end

function M.open_browser(cell_id)
  local cell = find_cell(cell_id)
  return cell and open_path(cell.data.html, options.open_cmd) or false
end

function M.yank(cell_id, register)
  local cell = find_cell(cell_id)
  if not cell then return false end
  local lines = cell.data.lines
  if type(lines) ~= "table" then
    lines = {}
    for _, segment in ipairs(segments(cell)) do
      if segment.kind ~= "image" then vim.list_extend(lines, text_lines(segment)) end
    end
  end
  if #lines == 0 then return false end
  vim.fn.setreg(register or '"', table.concat(lines, "\n"))
  return true
end

function M.toggle(cell_id, all)
  local cell = find_cell(cell_id)
  if not cell then return false end
  local state = buffers[cell.buf]
  if all and state then
    local any_visible = false
    for _, candidate in ipairs(state.order) do
      if not candidate.ui.hidden then any_visible = true break end
    end
    for _, candidate in ipairs(state.order) do
      candidate.ui.hidden = any_visible
      if any_visible then clear_virtual(candidate) else show_virtual(candidate, true) end
    end
    return true
  end
  cell.ui.hidden = not cell.ui.hidden
  if cell.ui.hidden then clear_virtual(cell) else show_virtual(cell, true) end
  return true
end

function M.refresh(buf)
  buf = number(buf, vim.api.nvim_get_current_buf())
  if buffers[buf] then
    refresh(buf, false)
    return
  end
  for _, state in pairs(buffers) do
    for _, cell in ipairs(state.order) do
      if cell.ui.float and cell.ui.float.buf == buf and valid_win(cell.ui.float.win) then
        render_float(cell, false)
        return
      end
    end
  end
end

return M
