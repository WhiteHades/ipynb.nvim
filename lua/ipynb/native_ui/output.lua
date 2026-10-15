-- Virtual output, floating windows, and their visibility/focus lifecycle.
return function(state, rich)
  local M = {}
  local valid_buf, valid_win, number = state.valid_buf, state.valid_win, state.number
  local current_positions, source_window = state.current_positions, state.source_window
  local output_ns = state.output_ns
  local mouse_maps = {}

  local function active_mouse_map(buf, mode)
    for _, map in ipairs(vim.list_extend(vim.api.nvim_buf_get_keymap(buf, mode), vim.api.nvim_get_keymap(mode))) do
      if map.lhs == "<LeftMouse>" then return map end
    end
  end

  local function clicked_output(buf, mouse)
    if not mouse then return end
    local buffer = state.buffers[buf]
    if not buffer then return end
    for _, cell in ipairs(buffer.order) do
      local target = cell.ui.mouse_more
      if target and valid_win(target.win) and cell.ui.virt_mark and target.win == mouse.winid then
        local mark = vim.api.nvim_buf_get_extmark_by_id(buf, output_ns, cell.ui.virt_mark, { details = true })
        if #mark > 0 then
          local text = vim.api.nvim_buf_get_lines(buf, mark[1], mark[1] + 1, false)[1] or ""
          local pos = vim.fn.screenpos(target.win, mark[1] + 1, math.max(1, #text))
          local row
          if pos.row > 0 then
            row = pos.row
          else
            local info = vim.fn.getwininfo(target.win)[1]
            if info and mark[1] == info.topline - 2 then
              local top = vim.fn.screenpos(target.win, info.topline, 0)
              row = info.winrow - #((mark[3] and mark[3].virt_lines) or {}) + top.row - info.winrow - 1
            end
          end
          if row and mouse.screenrow == row + target.row then return cell end
        end
      end
    end
  end

  local function install_mouse(buf)
    local maps = mouse_maps[buf] or {}
    mouse_maps[buf] = maps
    for _, mode in ipairs({ "n", "i" }) do
      local existing = active_mouse_map(buf, mode)
      local map = maps[mode] or {}
      if existing and existing.callback ~= map.handler then map.fallback = existing end
      map.handler = function()
        local ok, mouse = pcall(vim.fn.getmousepos)
        local cell = ok and clicked_output(buf, mouse)
        if cell then
          vim.schedule(function() vim.cmd.stopinsert(); M.open(cell.id) end)
          return "<Ignore>"
        end
        local fallback = map.fallback
        if not fallback then return "<LeftMouse>" end
        if fallback.callback then return fallback.callback() end
        if fallback.expr == 1 or fallback.expr == true then
          local ok, result = pcall(vim.api.nvim_eval, fallback.rhs)
          if ok then return vim.api.nvim_replace_termcodes(tostring(result or ""), true, false, true) end
        end
        return vim.api.nvim_replace_termcodes(fallback.rhs or "<LeftMouse>", true, false, true)
      end
      maps[mode] = map
      vim.keymap.set(mode, "<LeftMouse>", map.handler, {
        buffer = buf, expr = true, silent = true, desc = "Open full notebook output",
      })
    end
  end

  local function output_end(cell)
    local first, last = current_positions(cell)
    if not first or not last then return end
    local buffer = state.buffers[cell.buf]
    local tick = vim.api.nvim_buf_get_changedtick(cell.buf)
    local spans
    if vim.b[cell.buf].ipynb_code_spans_tick == tick then
      if not buffer.layout_known_spans or buffer.layout_known_tick ~= tick then
        buffer.layout_known_tick = tick
        buffer.layout_known_spans = vim.b[cell.buf].ipynb_code_spans
      end
      spans = buffer.layout_known_spans
    else
      if buffer.layout_tick ~= tick then
        buffer.layout_tick = tick
        local ok, parsed = pcall(require("ipynb.cells").spans, cell.buf)
        buffer.layout_spans = ok and parsed or nil
      end
      spans = buffer.layout_spans
    end
    if spans then
      local low, high = 1, #spans
      while low <= high do
        local middle = math.floor((low + high) / 2)
        if spans[middle].begin[1] <= first.line then low = middle + 1 else high = middle - 1 end
      end
      local span = spans[high]
      if span and last.line <= span["end"][1] then
        return { line = span["end"][1], col = span["end"][2] }
      end
    end
    return last
  end

  function M.clear_virtual(cell)
    rich.clear_images(cell, "virt")
    if cell.ui.virt_mark and valid_buf(cell.buf) then
      pcall(vim.api.nvim_buf_del_extmark, cell.buf, output_ns, cell.ui.virt_mark)
    end
    cell.ui.virt_mark = nil
    cell.ui.virt_key = nil
    cell.ui.mouse_more = nil
  end

  function M.close_float(cell, restore_source)
    rich.clear_images(cell, "float")
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

  local function cover_offset(buf, anchor)
    if not state.options.cover_empty_lines then return 0 end
    local offset, line = 0, anchor.line
    while line > 0 do
      local text = vim.api.nvim_buf_get_lines(buf, line, line + 1, false)[1] or ""
      local covered = text == ""
      for _, prefix in ipairs(state.options.cover_lines_starting_with or {}) do
        if vim.startswith(text, prefix) then covered = true break end
      end
      if not covered then return offset end
      line, offset = line - 1, offset - 1
    end
    return 0
  end

  function M.show_virtual(cell, force)
    if not state.options.virt_text_output or cell.ui.hidden or (cell.ui.float and valid_win(cell.ui.float.win)) then
      M.clear_virtual(cell)
      return
    end
    local end_pos = output_end(cell)
    if not end_pos then return end
    local win = source_window(cell.buf)
    if not valid_win(win) then return end
    local info = vim.fn.getwininfo(win)[1]
    if not info then return end
    local width = info.width - info.textoff
    if width < 1 then M.clear_virtual(cell) return end
    local anchor = { line = end_pos.line + cover_offset(cell.buf, end_pos), col = 0 }
    local count = vim.api.nvim_buf_line_count(cell.buf)
    if state.options.virt_lines_off_by_1 and anchor.line < count - 1 then anchor.line = anchor.line + 1 end
    anchor.line = math.max(0, math.min(anchor.line, math.max(0, count - 1)))
    local api = rich.provider()
    local layout = api.layout_key and api.layout_key() or {}
    local key = table.concat({ cell.revision, state.config_revision, win, anchor.line, width, info.height, vim.inspect(layout) }, ":")
    if not force and cell.ui.virt_mark and cell.ui.virt_key == key then return end
    local lines = rich.render_body(cell, {
      row = anchor.line,
      padding_top = 1,
      width = width,
      height = info.height,
    }, cell.buf, win, true)
    cell.ui.mouse_more = nil
    for index, line in ipairs(lines) do
      if line:find("More lines", 1, true) then cell.ui.mouse_more = { win = win, row = index }; break end
    end
    local virt_lines = { { { string.rep(" ", width), "Normal" } } }
    for index, line in ipairs(lines) do
      local group = (index == 1 or index == #lines) and rich.border_group(cell) or "IpynbVirtualText"
      virt_lines[#virt_lines + 1] = line:match("^│.*│$")
        and { { "│", rich.border_group(cell) }, { line:sub(4, -4), group }, { "│", rich.border_group(cell) } }
        or { { line, group } }
    end
    if cell.ui.mouse_more then cell.ui.mouse_more.row = cell.ui.mouse_more.row + 1 end
    local mark_options = { virt_lines = virt_lines, strict = false }
    if cell.ui.virt_mark then mark_options.id = cell.ui.virt_mark end
    cell.ui.virt_mark = vim.api.nvim_buf_set_extmark(cell.buf, output_ns, anchor.line, 0, mark_options)
    cell.ui.virt_key = key
    for _, record in ipairs(rich.image_records(cell, "virt")) do pcall(record.api.render, record.id) end
    if #rich.image_records(cell, "virt") > 0 then rich.image_canvas(win, true) end
    if api.refresh then pcall(api.refresh) end
    if cell.ui.mouse_more then install_mouse(cell.buf) end
  end

  local function source_relative_row(win, row)
    local info = vim.fn.getwininfo(win)[1]
    if not info then return end
    local count = vim.api.nvim_buf_line_count(vim.api.nvim_win_get_buf(win))
    local pos = vim.fn.screenpos(win, math.min(count, row + 1), 1)
    if not pos or pos.row == 0 then return end
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
    if not state.options.output_virt_lines and not state.options.cover_empty_lines then return end
    local end_pos = output_end(cell)
    if not end_pos then return end
    local row = end_pos.line + cover_offset(cell.buf, end_pos)
    if state.options.virt_lines_off_by_1 then row, height = row + 1, height - 1 end
    row = math.max(0, math.min(row, math.max(0, vim.api.nvim_buf_line_count(cell.buf) - 1)))
    local virtual = {}
    for _ = 1, math.max(0, height) do virtual[#virtual + 1] = { { "", "Normal" } } end
    local mark_options = { virt_lines = virtual, strict = false }
    if cell.ui.reserve_mark then mark_options.id = cell.ui.reserve_mark end
    cell.ui.reserve_mark = vim.api.nvim_buf_set_extmark(cell.buf, output_ns, row, 0, mark_options)
  end

  function M.render_float(cell, focus, reveal)
    local end_pos = output_end(cell)
    if not end_pos then return false end
    local float = cell.ui.float
    local source = float and float.source_win or source_window(cell.buf)
    if not valid_win(source) or vim.api.nvim_win_get_buf(source) ~= cell.buf then return false end
    local info = vim.fn.getwininfo(source)[1]
    if not info then return false end

    local border = rich.border_with_highlight(vim.deepcopy(state.options.output_win_border), cell)
    local border_width, border_height = border_size(border)
    local anchor_row = source_relative_row(source, end_pos.line)
    local row = anchor_row and anchor_row + 2 or info.height
    if reveal and row + border_height >= info.height and info.height > border_height + 2 then
      vim.api.nvim_win_call(source, function()
        vim.api.nvim_win_set_cursor(source, { end_pos.line + 1, 0 })
        vim.cmd("normal! zt")
      end)
      anchor_row = source_relative_row(source, end_pos.line)
      row = anchor_row and anchor_row + 2 or info.height
    end
    local screen_height = info.height - row - border_height
    local available_height = math.min(screen_height, number(state.options.output_win_max_height, 999999))
    local available_width = math.min(info.width - border_width, number(state.options.output_win_max_width, 999999))
    local gutter = state.options.output_win_cover_gutter and 0 or info.textoff
    available_width = available_width - gutter
    if available_width < 1 or available_height < 1 then
      if float and valid_win(float.win) and available_width > 0 and info.height > border_height + 2
        and number(state.options.output_win_max_height, 999999) > 0 then
        rich.clear_images(cell, "float")
        vim.api.nvim_win_set_config(float.win, { hide = true })
        cell.ui.float_key = nil
        return false
      end
      M.close_float(cell, true)
      M.show_virtual(cell, true)
      return false
    end

    local api = rich.provider()
    local layout = api.layout_key and api.layout_key() or {}
    local float_key = table.concat({
      cell.revision, state.config_revision, source, end_pos.line, end_pos.col,
      info.width, info.height, info.topline, vim.inspect(layout),
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

    M.clear_virtual(cell)
    local lines = rich.render_body(cell, {
      row = 0,
      width = available_width,
      height = available_height,
    }, display_buf, float and float.win or nil, false)
    vim.api.nvim_buf_set_lines(display_buf, 0, -1, false, lines)
    vim.api.nvim_buf_clear_namespace(display_buf, output_ns, 0, -1)
    vim.api.nvim_buf_set_extmark(display_buf, output_ns, 0, 0, {
      end_col = #lines[1], hl_group = rich.border_group(cell),
    })

    local height = math.min(available_height, #lines)
    local cropped = height == screen_height and #lines > height
    if cropped and state.options.output_crop_border and type(border) == "table" and #border > 0 then
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
      hide = false,
      zindex = number(state.options.output_win_zindex, 50),
    }
    if state.options.output_win_style then win_config.style = state.options.output_win_style end
    if state.options.output_show_more and not cropped and #lines > height then
      win_config.footer = { { (" 󰁅 %d More Lines "):format(#lines - height), "IpynbOutputFooter" } }
      win_config.footer_pos = "left"
    end

    if float and valid_win(float.win) then
      vim.api.nvim_win_set_config(float.win, win_config)
    else
      local ok, win = pcall(vim.api.nvim_open_win, display_buf, false, win_config)
      if not ok then
        pcall(vim.api.nvim_buf_delete, display_buf, { force = true })
        state.notify("Unable to display notebook output")
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
    elseif state.options.floating_window_focus == "bottom" then
      pcall(vim.api.nvim_win_set_cursor, float.win, { vim.api.nvim_buf_line_count(display_buf), 0 })
    else
      pcall(vim.api.nvim_win_set_cursor, float.win, { 1, 0 })
    end

    reserve_float_space(cell, #lines + border_height + 1)
    for _, record in ipairs(rich.image_records(cell, "float")) do pcall(record.api.render, record.id) end
    if api.refresh then pcall(api.refresh) end
    cell.ui.float_key = float_key
    if focus and valid_win(float.win) then
      vim.cmd(("noautocmd call nvim_set_current_win(%d)"):format(float.win))
    end
    return true
  end

  function M.open(cell_id)
    local cell = state.find_cell(cell_id)
    if not cell then return false end
    local behavior = state.options.enter_output_behavior
    local existed = cell.ui.float and valid_win(cell.ui.float.win)
    if not existed and behavior == "no_open" then return false end
    cell.ui.hidden = false
    local focus = existed or behavior == "open_and_enter"
    return M.render_float(cell, focus, true)
  end

  function M.show(cell_id)
    local cell = state.find_cell(cell_id)
    if not cell then return false end
    cell.ui.hidden = false
    return M.render_float(cell, false, true)
  end

  function M.hide(explicit, cell_id)
    local hidden = false
    local target = explicit and state.find_cell(cell_id) or nil
    if target then
      target.ui.hidden = true
      M.clear_virtual(target)
      hidden = true
    end
    for _, buffer in pairs(state.buffers) do
      for _, cell in ipairs(buffer.order) do
        if cell.ui.float and valid_win(cell.ui.float.win) then
          hidden = M.close_float(cell, true) or hidden
          if not cell.ui.hidden then M.show_virtual(cell, true) end
        end
      end
    end
    return hidden
  end

  function M.toggle(cell_id, all)
    local cell = state.find_cell(cell_id)
    if not cell then return false end
    local buffer = state.buffers[cell.buf]
    if all and buffer then
      local any_visible = false
      for _, candidate in ipairs(buffer.order) do
        if not candidate.ui.hidden then any_visible = true break end
      end
      for _, candidate in ipairs(buffer.order) do
        candidate.ui.hidden = any_visible
        if any_visible then M.clear_virtual(candidate) else M.show_virtual(candidate, true) end
      end
      return true
    end
    cell.ui.hidden = not cell.ui.hidden
    if cell.ui.hidden then M.clear_virtual(cell) else M.show_virtual(cell, true) end
    return true
  end

  return M
end
