-- The output layout owns padding. Measure and render the same geometry, and
-- keep each buffer/placement separate even when they share an image file.
local ok, image = pcall(require, "image")
local images = {}
local api = {}
local term
local backend_initialized = false
local INLINE_WIDTH_FACTOR = 0.50
local INLINE_HEIGHT_FACTOR = 0.35

-- image.nvim derives float bounds from screenpos(win, 1, 1), which becomes
-- zero when line 1 is scrolled away. Notebook floats have real scrollable rows.
if ok then
  local loaded, utils = pcall(require, "image.utils")
  if loaded then
    -- Use the same accessor as image.nvim's renderer and crop backend. Lua's
    -- slash/dot module names otherwise create independent terminal caches.
    term = utils.term
    local get_window = utils.window.get_window
    utils.window.get_window = function(win, opts)
      local window = get_window(win, opts)
      if window and window.is_floating and vim.bo[window.buffer].filetype == "ipynb_output" then
        local info = vim.fn.getwininfo(win)[1]
        local origin = vim.fn.screenpos(win, info.topline, 1)
        window.rect = {
          top = origin.row - 1, left = origin.col - 1,
          bottom = origin.row + info.height - 2,
          right = origin.col + info.width - 1,
        }
      end
      return window
    end
  else
    term = require("image.utils.term")
  end
  require("ipynb.image_terminal").attach(term, function()
    if vim.fn.exists("*IpynbUpdateInterface") == 1 then
      vim.fn.IpynbUpdateInterface()
    end
  end)
end

local function covered_by_float(win, bounds)
  local source = vim.fn.getwininfo(win)[1]
  if bounds then
    source = {winrow=bounds.top + 1, wincol=bounds.left + 1,
      height=bounds.bottom - bounds.top + 1, width=bounds.right - bounds.left}
  end
  local source_z = vim.api.nvim_win_get_config(win).zindex or 0
  for _, other in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if other ~= win then
      local config = vim.api.nvim_win_get_config(other)
      local info = vim.fn.getwininfo(other)[1]
      if config.relative ~= "" and not config.hide and (config.zindex or 50) > source_z and info.winrow < source.winrow + source.height
        and info.winrow + info.height > source.winrow
        and info.wincol < source.wincol + source.width
        and info.wincol + info.width > source.wincol then return true end
    end
  end
  return false
end

local function positive(value)
  return type(value) == "number" and value > 0 and value < math.huge
end

local function render_prepared(img)
  local info = vim.fn.getwininfo(img.window)[1]
  local floating = vim.api.nvim_win_get_config(img.window).relative ~= ""
  local x, y, bounds
  if floating then
    local origin = vim.fn.screenpos(img.window, info.topline, 1)
    x = origin.col - 1 + img.geometry.x
    y = origin.row + img.ipynb_row - info.topline
    bounds = { left = origin.col - 1, right = origin.col - 1 + info.width,
      top = origin.row - 1, bottom = origin.row + info.height - 2 }
  else
    if vim.api.nvim_win_call(img.window, function() return vim.fn.foldclosed(img.ipynb_row + 1) end) ~= -1 then
      img:clear()
      return
    end
    x = info.wincol - 1 + info.textoff + img.geometry.x
    local pos = vim.fn.screenpos(img.window, img.ipynb_row + 1, 1)
    if pos.row > 0 then
      y = pos.row + img.render_offset_top
    elseif img.ipynb_virtual_layout and img.ipynb_row == info.topline - 2 then
      -- topfill counts the entire output, not just this bitmap. Keep captions,
      -- other plots and bottom padding out of the image's scroll calculation.
      local diff = vim.fn.screenpos(img.window, info.topline, 1).row - info.winrow
      y = info.winrow - img.ipynb_virtual_layout.virtual_height + diff - 1 + img.render_offset_top
    else
      img:clear()
      return
    end
    bounds = { left = info.wincol - 1 + info.textoff, right = info.wincol - 1 + info.width,
      top = info.winrow - 1, bottom = info.winrow + info.height - 2 }
  end
  local width, height = img.geometry.width, img.geometry.height
  if y + height <= bounds.top or y > bounds.bottom then img:clear(); return end
  if covered_by_float(img.window, {left=x, right=x+width, top=y, bottom=y+height-1}) then
    img:clear()
    return
  end
  img.bounds = bounds
  img.global_state.backend.render(img, x, y, width, height)
  img.rendered_geometry = { x = x, y = y, width = width, height = height }
end

api.from_file = function(path, opts)
  if not ok then return nil end
  opts = opts or {}
  if opts.window == vim.NIL then opts.window = nil end
  local id = opts.id or path
  local img = images[id]
  if img and img.ipynb_path ~= path then api.clear(id); img = nil end
  if not img then
    local loaded, result = pcall(image.from_file, path, opts)
    if not loaded or not result then return nil end
    img = result
    images[id] = img
    img.ipynb_source_width, img.ipynb_source_height = img.image_width, img.image_height
    require("ipynb.image_kitty").track(img, term)
    local render = img.render
    -- Guard provider-triggered redraws and asynchronous conversion callbacks too.
    img.render = function(self, geometry)
      if self.ipynb_active == false or not self.window or not vim.api.nvim_win_is_valid(self.window) then return end
      if not self.global_state.enabled or self.global_state.disable_decorator_handling then return end
      if vim.api.nvim_win_get_buf(self.window) ~= self.buffer then self:clear(); return end
      if vim.api.nvim_win_get_tabpage(self.window) ~= vim.api.nvim_get_current_tabpage() then self:clear(); return end
      if self.ipynb_row >= vim.api.nvim_buf_line_count(self.buffer) then self:clear(); return end
      local config = vim.api.nvim_win_get_config(self.window)
      if api.panel_supported() and self.geometry.width and self.geometry.width > 0 then
        local size = term.get_size()
        local width = math.floor(self.geometry.width * size.cell_width + 0.5)
        local height = math.floor(self.geometry.height * size.cell_height + 0.5)
        local _, _, radius = require("ipynb.image_style").metrics(size)
        local key = table.concat({ width, height, radius }, ":")
        if self.ipynb_prepared ~= key then
          if self.ipynb_preparing ~= key then
            self.ipynb_preparing = key
            self:clear()
            require("ipynb.image_style").rounded(self.ipynb_path, width, height, radius,
              self.ipynb_source_width, self.ipynb_source_height, function(path)
              if images[id] ~= self or self.ipynb_preparing ~= key then return end
              if not path then self:render(); return end
              self.path, self.resized_path, self.cropped_path = path, path, path
              -- Do not let image.nvim clone this padded canvas as the raw plot.
              self.original_path = path
              self.image_width, self.image_height = width, height
              self.source_format = "png"
              self.resize_hash, self.crop_hash, self.transform_key = nil, nil, nil
              self.transform_signature, self.pending_transform_key = nil, nil
              self.ipynb_prepared = key
              self:render()
            end)
          end
          return
        end
        render_prepared(self)
        return
      end
      if covered_by_float(self.window) then self:clear(); return end
      if config.relative ~= "" then
        -- The renderer adds one row to a float's rect.top. Convert the real
        -- buffer row to that convention after accounting for viewport scroll.
        self.geometry.y = self.ipynb_row - vim.fn.getwininfo(self.window)[1].topline
      end
      render(self, geometry)
    end
  end
  img.buffer, img.window = opts.buffer, opts.window
  img.geometry.x, img.geometry.y = opts.x or 0, opts.y or 0
  img.ipynb_path = path
  img.ipynb_viewer = opts.viewer
  img.ipynb_virtual_layout = opts.virtual_layout
  img.ipynb_row = opts.y or 0
  img.render_offset_top = opts.render_offset_top or 0
  img.with_virtual_padding = false
  img.ipynb_bounds = { width = opts.max_width, height = opts.max_height }
  -- Apply provider limits once during measurement, not again against the
  -- smaller output float during rendering.
  img.ignore_global_max_size = true
  if opts.buffer and not opts.viewer and not vim.b[opts.buffer].ipynb_image_mouse then
    vim.b[opts.buffer].ipynb_image_mouse = true
    local function click()
      local mouse = vim.fn.getmousepos()
      for _, candidate in pairs(images) do
        local rect = candidate.rendered_geometry
        if candidate.ipynb_active and candidate.is_rendered and rect
          and candidate.window == mouse.winid
          and mouse.screencol - 1 >= rect.x and mouse.screencol - 1 < rect.x + rect.width
          and mouse.screenrow - 1 >= rect.y and mouse.screenrow - 1 < rect.y + rect.height then
          local path = candidate.ipynb_path
          vim.schedule(function()
            vim.cmd.stopinsert()
            require("ipynb.image_viewer").open(path)
          end)
          return "<Ignore>"
        end
      end
      return "<LeftMouse>"
    end
    for _, mode in ipairs({ "n", "i" }) do
      local existing = vim.api.nvim_buf_call(opts.buffer, function()
        return vim.fn.maparg("<LeftMouse>", mode)
      end)
      if existing == "" then
        vim.keymap.set(mode, "<LeftMouse>", click,
          { buffer = opts.buffer, expr = true, silent = true, desc = "View notebook image" })
      end
    end
  end
  return id
end

api.layout_key = function()
  local size = term and term.get_size()
  return size and { size.cell_width, size.cell_height } or {}
end

api.panel_supported = function()
  if not ok or vim.fn.executable("magick") ~= 1 then return false end
  local style = package.loaded["ipynb.image_style"]
  if style and style.failed then return false end
  if not backend_initialized then
    -- image.nvim defers backend setup until its first operation, even when a
    -- notebook has only code/text. This nonexistent ID cannot clear user images.
    backend_initialized = pcall(image.clear, "ipynb-panel-init-" .. vim.uv.os_getpid())
  end
  local loaded, backend = pcall(require, "image/backends/kitty")
  local state = loaded and backend.state
  local size = term and term.get_size()
  return state and state.options.backend == "kitty" and state.options.kitty_method == "normal"
    and size and positive(size.cell_width) and positive(size.cell_height) or false
end

api.image_size = function(id)
  local img = images[id]
  local size = term and term.get_size()
  if not img or not size or not positive(size.cell_width) or not positive(size.cell_height)
    or not positive(img.ipynb_source_width) or not positive(img.ipynb_source_height) then
    if img then img.geometry.width, img.geometry.height = 0, 0 end
    return { width = 0, height = 0 }
  end
  local bounds = img.ipynb_bounds
  local opts = img.global_state.options
  local width, height = bounds.width, bounds.height
  if not positive(width) or not positive(height) then
    img.geometry.width, img.geometry.height = 0, 0
    return { width = 0, height = 0 }
  end
  if not img.ipynb_viewer then
    if img.render_offset_top > 0 then
      -- Notebook previews own their bounds; Markdown's percentage caps should
      -- not shrink plots a second time. Leave room for code around the image.
      width = math.max(1, math.floor(width * INLINE_WIDTH_FACTOR))
      height = math.max(1, math.floor(height * INLINE_HEIGHT_FACTOR))
    else
      if positive(opts.max_width_window_percentage) then
        width = math.min(width, math.max(1, math.floor(bounds.width * opts.max_width_window_percentage / 100)))
      end
      if positive(opts.max_height_window_percentage) then
        height = math.min(height, math.max(1, math.floor(bounds.height * opts.max_height_window_percentage / 100)))
      end
    end
    if positive(opts.max_width) then width = math.min(width, opts.max_width) end
    if positive(opts.max_height) then height = math.min(height, opts.max_height) end
  end
  local scale = math.min(
    width * size.cell_width / img.ipynb_source_width,
    height * size.cell_height / img.ipynb_source_height
  )
  -- Keep fractional cell geometry for pixel aspect ratio; reserve whole rows.
  -- Avoid ceil(23.000000000000004) reserving a spurious row at the bound.
  img.geometry.width = math.min(width, img.ipynb_source_width * scale / size.cell_width)
  img.geometry.height = math.min(height, img.ipynb_source_height * scale / size.cell_height)
  if api.panel_supported() then
    -- Fit the original aspect ratio inside a whole-cell transparent canvas.
    -- Explicit Kitty c/r sizing must not stretch a fractional last row/column.
    img.geometry.width, img.geometry.height = math.ceil(img.geometry.width), math.ceil(img.geometry.height)
  end
  return { width = math.ceil(img.geometry.width), height = math.ceil(img.geometry.height) }
end

local function render(id)
  local img = images[id]
  if not img then return end
  if not img.window or not vim.api.nvim_win_is_valid(img.window)
    or vim.api.nvim_win_get_buf(img.window) ~= img.buffer then
    img.window = vim.fn.bufwinid(img.buffer)
  end
  if img.window ~= -1 and vim.api.nvim_win_is_valid(img.window) then
    if img.geometry.width and img.geometry.width > 0 then img:render() end
  end
end

api.clear = function(id)
  if images[id] then
    images[id].ipynb_active = false
    images[id]:clear()
    images[id] = nil
  end
end

local refresh_pending = false
api.refresh = function()
  if refresh_pending then return end
  refresh_pending = true
  vim.schedule(function()
    refresh_pending = false
    if not next(images) or vim.v.exiting ~= vim.NIL then return end
    -- Apply virtual-line changes before measuring their screen positions.
    vim.cmd("redraw")
    for id, img in pairs(images) do
      if img.ipynb_active then render(id) end
    end
  end)
end

local float_layout
vim.api.nvim_set_decoration_provider(vim.api.nvim_create_namespace("ipynb_image_masks"), {
  on_end = function()
    if not next(images) then return end
    local floats = {}
    for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
      local config = vim.api.nvim_win_get_config(win)
      if config.relative ~= "" and not config.hide then
        local info = vim.fn.getwininfo(win)[1]
        floats[#floats + 1] = table.concat({win, info.winrow, info.wincol, info.width, info.height, config.zindex or 50}, ":")
      end
    end
    local layout = table.concat(floats, ";")
    if layout ~= float_layout then
      float_layout = layout
      -- WhichKey suppresses window events while creating its popup.
      api.refresh()
    end
  end,
})

api.render = function(id)
  if not images[id] then return end
  images[id].ipynb_active = true
  api.refresh()
end

vim.api.nvim_create_autocmd({ "WinScrolled", "WinNew", "WinClosed", "WinResized", "BufWinEnter", "FocusGained", "TabEnter" }, {
  group = vim.api.nvim_create_augroup("ipynb_image_placements", { clear = true }),
  callback = function()
    vim.schedule(api.refresh)
  end,
})

api.clear_all = function()
  for id in pairs(images) do api.clear(id) end
  images = {}
end

return { image_api = api }
