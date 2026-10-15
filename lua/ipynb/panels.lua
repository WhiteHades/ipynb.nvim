-- Pixel borders share image.nvim's transport, focus handling and cleanup.
local M = {}
local specs = {}
local scheduled = false
local api

function M.available()
  if vim.g.ipynb_image_provider and vim.g.ipynb_image_provider ~= "image.nvim" then return false end
  if not api then
    local loaded, adapter = pcall(require, "ipynb.molten_load_image_nvim")
    if not loaded then return false end
    api = adapter.image_api
  end
  return api.panel_supported and api.panel_supported() or false
end

local function remove(key)
  local spec = specs[key]
  if not spec then return end
  specs[key] = nil
  for _, image in pairs(spec.images or {}) do api.clear(image) end
end

function M.remove(key)
  remove(key)
end

local function text_rows(win, first, last)
  if last < first then return 0 end
  local height = vim.api.nvim_win_text_height(win, { start_row = first, end_row = last })
  return height.all - height.fill
end

local function rect(spec, win)
  local info = vim.fn.getwininfo(win)[1]
  local x, y, width, height
  if spec.float then
    -- getwininfo starts at the float's outer border, unlike screenpos.
    x, y, width, height = info.wincol - 1, info.winrow - 1, info.width + 2, info.height + 2
  else
    local count = vim.api.nvim_buf_line_count(spec.buf)
    if spec.first >= count or (spec.last and spec.last >= count) then return end
    x, width = info.wincol + info.textoff - 1, math.min(spec.width, info.width - info.textoff)
    local pos = vim.fn.screenpos(win, spec.first + 1, 1)
    if spec.last then
      height = text_rows(win, spec.first, spec.last)
      if pos.row > 0 then y = pos.row - 1
      elseif spec.first < info.topline - 1 then
        y = info.winrow - 1 - text_rows(win, spec.first, info.topline - 2)
        local skip = vim.api.nvim_win_call(win, function() return vim.fn.winsaveview().skipcol end)
        if skip > 0 then
          local remaining = vim.api.nvim_win_text_height(win, {
            start_row = info.topline - 1, end_row = info.topline - 1, start_vcol = skip,
          })
          y = y - text_rows(win, info.topline - 1, info.topline - 1) + remaining.all
        end
      else return end
    else
      height = spec.height
      if pos.row > 0 then y = pos.row
      elseif spec.first == info.topline - 2 then
        local diff = vim.fn.screenpos(win, info.topline, 0).row - info.winrow
        y = info.winrow - height + diff - 1
      else return end
    end
  end
  local top = spec.float and 0 or info.winrow - 1
  local bottom = spec.float and vim.o.lines - vim.o.cmdheight or info.winrow + info.height - 1
  local visible_top, visible_bottom = math.max(y, top), math.min(y + height, bottom)
  if width < 4 or visible_top >= visible_bottom then return end
  return { x = x, y = visible_top, width = width, height = visible_bottom - visible_top,
    full_height = height, crop = visible_top - y,
    bounds = { left = x, right = x + width, top = visible_top, bottom = visible_bottom - 1 } }
end

local function right_label(spec)
  if not spec.last then return 0 end
  local namespace = vim.api.nvim_get_namespaces()["ipynb-cell-header"]
  if not namespace then return 0 end
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(spec.buf, namespace, { spec.first, 0 }, { spec.first, -1 }, { details = true })) do
    local width = 0
    for _, chunk in ipairs(mark[4].virt_text or {}) do width = width + vim.fn.strdisplaywidth(chunk[1]) end
    return width
  end
  return 0
end

local function draw(key, spec, win)
  if vim.api.nvim_win_get_tabpage(win) ~= vim.api.nvim_get_current_tabpage() then
    if spec.keys then spec.keys[win] = nil end
    if spec.images and spec.images[win] then api.clear(spec.images[win]); spec.images[win] = nil end
    return
  end
  local shape = rect(spec, win)
  spec.images = spec.images or {}
  if not shape then
    if spec.keys then spec.keys[win] = nil end
    if spec.images[win] then api.clear(spec.images[win]) end
    spec.images[win] = nil
    return
  end
  local size = require("image.utils").term.get_size()
  local cw, ch = size.cell_width, size.cell_height
  local radius, stroke = require("ipynb.image_style").metrics(size)
  local width, height = math.floor(shape.width * cw), math.floor(shape.full_height * ch)
  local full_height = shape.full_height * ch
  local left, right = cw / 2, width - cw / 2
  local top, bottom = ch / 2, full_height - ch / 2
  radius = math.min(radius, (right - left) / 2, (bottom - top) / 2)
  local color = vim.api.nvim_get_hl(0, { name = spec.group, link = false }).fg
    or vim.api.nvim_get_hl(0, { name = "Normal", link = false }).fg
  if not color then return end
  local gaps = {}
  if spec.label and spec.label > 0 then gaps[#gaps + 1] = { 2 * cw, (2 + spec.label) * cw } end
  if spec.center_label then
    gaps[#gaps + 1] = { (shape.width - spec.center_label) * cw / 2, (shape.width + spec.center_label) * cw / 2 }
  end
  local label = right_label(spec)
  if label > 0 then gaps[#gaps + 1] = { width - label * cw, width - 2 * cw } end
  -- Draw the top edge in pieces so the native, selectable labels stay legible.
  local path = ("M %g,%g"):format(left + radius, top)
  local cursor = left + radius
  for _, gap in ipairs(gaps) do
    local first, last = math.max(cursor, gap[1]), math.min(right - radius, gap[2])
    if last > first then
      path = path .. (" L %g,%g M %g,%g"):format(first, top, last, top)
      cursor = last
    end
  end
  path = path .. (" L %g,%g Q %g,%g %g,%g L %g,%g Q %g,%g %g,%g L %g,%g Q %g,%g %g,%g L %g,%g Q %g,%g %g,%g")
    :format(right - radius, top, right, top, right, top + radius,
      right, bottom - radius, right, bottom, right - radius, bottom,
      left + radius, bottom, left, bottom, left, bottom - radius,
      left, top + radius, left, top, left + radius, top)
  local command = { "-size", ("%dx%d"):format(width, height), "xc:none", "-fill", "none", "-stroke", ("#%06x"):format(color),
    "-strokewidth", tostring(stroke), "-draw", "path '" .. path .. "'" }
  local signature = vim.inspect(command)
  spec.keys = spec.keys or {}
  local request = signature .. ":" .. vim.inspect(shape)
  if spec.keys[win] == request then return end
  spec.keys[win] = request
  require("ipynb.image_style").generate(signature, command, function(file)
    if not file or specs[key] ~= spec or not vim.api.nvim_win_is_valid(win) or spec.keys[win] ~= request
      or vim.api.nvim_win_get_buf(win) ~= spec.buf then return end
    -- Kitty crops the cached full border as it moves through the viewport.
    local frame = vim.tbl_extend("force", shape, { y = shape.y - shape.crop, height = shape.full_height })
    local identifier = api.from_file(file, { id = key .. ":" .. win, buffer = spec.buf, window = win, frame = frame })
    if identifier then spec.images[win] = identifier; api.render(identifier)
    else spec.keys[win] = nil end
  end)
end

function M.refresh()
  if not M.available() then return end
  for key, spec in pairs(specs) do
    if not vim.api.nvim_buf_is_valid(spec.buf) then remove(key)
    elseif spec.win then
      if not vim.api.nvim_win_is_valid(spec.win) then remove(key)
      elseif vim.api.nvim_win_get_buf(spec.win) == spec.buf then draw(key, spec, spec.win)
      else
        if spec.keys then spec.keys[spec.win] = nil end
        if spec.images and spec.images[spec.win] then api.clear(spec.images[spec.win]); spec.images[spec.win] = nil end
      end
    else
      local visible = {}
      for _, win in ipairs(vim.fn.win_findbuf(spec.buf)) do visible[win] = true; draw(key, spec, win) end
      for win, image in pairs(spec.images or {}) do
        if not visible[win] then
          if spec.keys then spec.keys[win] = nil end
          api.clear(image)
          spec.images[win] = nil
        end
      end
      for win in pairs(spec.keys or {}) do if not visible[win] then spec.keys[win] = nil end end
    end
  end
end

local function schedule()
  if scheduled then return end
  scheduled = true
  vim.schedule(function()
    scheduled = false
    if not next(specs) or vim.v.exiting ~= vim.NIL then return end
    vim.cmd("redraw")
    M.refresh()
  end)
end

function M.code(buf, ranges)
  local prefix = "ipynb-panel-code-" .. buf .. "-"
  for key in pairs(specs) do if key:sub(1, #prefix) == prefix then remove(key) end end
  if not M.available() then return end
  for index, range in ipairs(ranges) do
    range.buf, range.group = buf, "IpyNotebookCodeBorder"
    specs[prefix .. index] = range
  end
  schedule()
end

function M.output(key, spec)
  remove(key)
  if not M.available() then return end
  specs[key] = spec
  schedule()
end

function M.clear_all()
  for key in pairs(specs) do remove(key) end
end

function M.fallback()
  if not next(specs) then return end
  M.clear_all()
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if vim.b[buf].ipynb_markdown_enabled == true then
      vim.b[buf].ipynb_markdown_render_tick = nil
      require("ipynb.markdown").refresh(buf)
      require("ipynb.cell_header").refresh(buf)
    end
  end
  local ui = package.loaded["ipynb.native_ui"]
  if ui then ui.configure({}) end
end

vim.api.nvim_create_autocmd({ "WinScrolled", "WinResized", "VimResized", "WinClosed", "BufWinEnter", "BufWinLeave", "FocusGained", "TabEnter", "ColorScheme" }, {
  group = vim.api.nvim_create_augroup("ipynb_panels", { clear = true }), callback = schedule,
})

return M
