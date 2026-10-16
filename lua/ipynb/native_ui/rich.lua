-- Output contents and image-provider resources; notebook state stays with the caller.
return function(state)
  local M = {}
  local number, notify = state.number, state.notify
  local saved_fillchars = {}

  function M.image_canvas(win, enabled)
    if not state.valid_win(win) then
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

  local function safe_id(value)
    return tostring(value):gsub("[^%w_.-]", "_")
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
      local snacks = ok and module and module.snacks_api or none_api
      local handles, used = {}, {}
      providers["snacks.nvim"] = {
        from_file = function(path, opts)
          local api = opts.virtual_layout and image_nvim_api() or snacks
          used[api] = true
          local id = api.from_file(path, opts)
          if not id then return nil end
          local handle = {}
          handles[handle] = { api = api, id = id }
          return handle
        end,
        image_size = function(handle)
          local record = handles[handle]
          return record and record.api.image_size(record.id) or { width = 0, height = 0 }
        end,
        render = function(handle)
          local record = handles[handle]
          if record then record.api.render(record.id) end
        end,
        clear = function(handle)
          local record = handles[handle]
          if record then
            record.api.clear(record.id)
            handles[handle] = nil
          end
        end,
        clear_all = function()
          for api in pairs(used) do
            if api.clear_all then
              api.clear_all()
            else
              for handle, record in pairs(handles) do
                if record.api == api then
                  api.clear(record.id)
                  handles[handle] = nil
                end
              end
            end
          end
          handles, used = {}, {}
        end,
        refresh = function() image_nvim_api().refresh() end,
        layout_key = function() return image_nvim_api().layout_key() end,
      }
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
          image_pane = bridge.wezterm_ipynb_init(initial_pane, state.options.split_direction, state.options.split_size)
        end
        bridge.send_image(path, tostring(image_pane), tostring(initial_pane))
      end,
    }
    return providers.wezterm
  end

  function M.provider()
    if state.options.image_provider == "image.nvim" then return image_nvim_api() end
    if state.options.image_provider == "snacks.nvim" then return snacks_api() end
    if state.options.image_provider == "wezterm" then return wezterm_api() end
    return none_api
  end

  function M.image_records(cell, kind)
    cell.ui.images[kind] = cell.ui.images[kind] or {}
    return cell.ui.images[kind]
  end

  function M.clear_images(cell, kind)
    local records = M.image_records(cell, kind)
    for _, record in ipairs(records) do pcall(record.api.clear, record.id) end
    cell.ui.images[kind] = {}
    if state.shutting_down then return end
    local api = M.provider()
    if api.refresh then pcall(api.refresh) end
  end

  function M.shutdown()
    state.shutting_down = true
    local apis = {}
    for _, api in pairs(providers) do apis[api] = true end
    -- The popup can load an adapter without ever rendering notebook output.
    for module, field in pairs({
      ["ipynb.molten_load_image_nvim"] = "image_api",
      ["ipynb.molten_load_snacks_nvim"] = "snacks_api",
    }) do
      local loaded = package.loaded[module]
      if type(loaded) == "table" and loaded[field] then apis[loaded[field]] = true end
    end
    for api in pairs(apis) do if api.clear_all then pcall(api.clear_all) end end
    for _, buffer in pairs(state.buffers) do
      for _, cell in ipairs(buffer.order) do
        M.clear_images(cell, "virt")
        M.clear_images(cell, "float")
      end
    end
    local viewer = package.loaded["ipynb.image_viewer"]
    if type(viewer) == "table" and viewer.close then pcall(viewer.close) end
  end

  function M.border_group(cell)
    local group = "IpynbOutputBorder"
    if cell.data.status == "hold" then
      group = "IpynbOutputBorderQueued"
    elseif cell.data.status == "running" then
      group = "IpynbOutputBorderRunning"
    elseif cell.data.status == "done" and cell.data.success == false then
      group = "IpynbOutputBorderFail"
    elseif cell.data.old then
      group = "IpynbOutputBorderStale"
    elseif cell.data.status == "done" then
      group = "IpynbOutputBorderSuccess"
    end
    return group
  end

  function M.border_with_highlight(border, cell)
    if not state.options.use_border_highlights or type(border) ~= "table" then return border end
    local group = M.border_group(cell)
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
      label = "Queued"
    elseif status == "running" then
      label = "... Running"
    elseif status == "done" then
      label = cell.data.success == false and "✗ Failed" or "✓ Done"
    else
      return "Out[_]: Not run"
    end
    local old = cell.data.old and "  Previous" or ""
    local elapsed = ""
    if not cell.data.old and state.options.output_show_exec_time and cell.data.elapsed ~= nil then
      local value = tonumber(cell.data.elapsed)
      elapsed = value and string.format(" %.2fs", value) or (" " .. tostring(cell.data.elapsed))
    end
    return ("Out[%s]: %s%s%s"):format(execution, label, elapsed, old)
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
    if not state.options.wrap_output or width < 1 then return lines end
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

  local function more_lines_line(count, hint, width)
    local label = ("󰁅 %d More lines"):format(count)
    if not hint or hint == "" then return label end
    local suffix = " [" .. hint .. "]"
    if vim.fn.strdisplaywidth(label .. suffix) <= width then return label .. suffix end
    local compact = ("󰁅 %d"):format(count)
    local compact_hint = "[" .. hint .. "]"
    local available = width - vim.fn.strdisplaywidth(compact_hint) - 1
    if available >= vim.fn.strdisplaywidth(compact) then return compact .. " " .. compact_hint end
    return take_display_width("…" .. compact_hint, width)
  end

  local function truncate_lines(lines, hint, width)
    local maximum = math.max(1, number(state.options.virt_text_max_lines, 8) - 2)
    if #lines <= maximum then return lines end
    if type(hint) == "function" then hint = hint() end
    if state.options.virt_text_truncate == "top" and maximum > 2 then
      local result = { lines[1], more_lines_line(#lines - maximum, hint, width) }
      for index = #lines - maximum + 3, #lines do result[#result + 1] = lines[index] end
      return result, 2
    end
    local result = {}
    for index = 1, maximum - 1 do result[#result + 1] = lines[index] end
    result[#result + 1] = more_lines_line(#lines - maximum + 1, hint, width)
    return result, #result
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

  function M.render_body(cell, shape, target_buf, target_win, virtual)
    local kind = virtual and "virt" or "float"
    M.clear_images(cell, kind)
    local api = M.provider()
    local records = M.image_records(cell, kind)
    local body = {}
    local more_rows = {}
    local remaining = number(state.options.limit_output_chars, 1000000)
    local image_index = 0
    local inset = virtual and shape.width >= 4 and 1 or 0
    local content_width = math.max(1, shape.width - inset * 2)

    for _, segment in ipairs(segments(cell)) do
      if segment.kind == "image" then
        local location = state.options.image_location
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
            x = inset,
            y = virtual and shape.row or row,
            render_offset_top = virtual and (row + (shape.padding_top or 0)) or 0,
            virtual_layout = virtual and shape or nil,
            with_virtual_padding = false,
            max_width = content_width,
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
        if state.options.limit_output_chars and state.options.limit_output_chars > 0 then
          if #joined > remaining then
            joined = joined:sub(1, remaining) .. ("\n...truncated to %d chars"):format(state.options.limit_output_chars)
          end
          remaining = math.max(0, remaining - #joined)
        end
        lines = wrap_lines(vim.split(joined, "\n", { plain = true }), content_width)
        if virtual then
          local more_row
          lines, more_row = truncate_lines(lines, shape.output_toggle_hint, content_width)
          if more_row then more_rows[#more_rows + 1] = #body + more_row + 1 end
        end
        vim.list_extend(body, lines)
      end
    end

    local heading = header(cell)
    local title = (virtual and "╭─ " or " ") .. heading .. " "
    local title_width = math.max(0, shape.width - (virtual and 1 or 0))
    if vim.fn.strdisplaywidth(title) > title_width then title = take_display_width(title, title_width) end
    local lines = {
      title .. (virtual and (string.rep("─", math.max(0, title_width - vim.fn.strdisplaywidth(title)))
        .. (shape.width > 0 and "╮" or "")) or ""),
    }
    for _, line in ipairs(body) do
      local rail = inset > 0 and "│" or ""
      lines[#lines + 1] = rail .. pad(line, content_width) .. rail
    end
    if virtual then
      lines[#lines + 1] = (shape.width > 1 and "╰" or "") .. string.rep("─", math.max(0, shape.width - 2))
        .. (shape.width > 0 and "╯" or "")
    end
    shape.virtual_height = #lines + (virtual and (shape.padding_top or 0) or 0)
    return lines, records, more_rows
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
    local cell = state.find_cell(cell_id)
    if not cell then return false end
    local path
    for _, segment in ipairs(segments(cell)) do
      if segment.kind == "image" and type(segment.path) == "string" and segment.path ~= "" then
        path = segment.path
        break
      end
    end
    if not path then return false end
    if state.options.image_provider == "image.nvim" then
      return require("ipynb.image_viewer").open(path)
    end
    return open_path(path)
  end

  function M.open_browser(cell_id)
    local cell = state.find_cell(cell_id)
    return cell and open_path(cell.data.html, state.options.open_cmd) or false
  end

  function M.yank(cell_id, register)
    local cell = state.find_cell(cell_id)
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

  return M
end
