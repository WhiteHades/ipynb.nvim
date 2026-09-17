-- The output layout owns padding. Measure and render the same geometry, and
-- keep each buffer/placement separate even when they share an image file.
local ok, image = pcall(require, "image")
local images = {}
local api = {}
local term

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

local function covered_by_output(win)
  local source = vim.fn.getwininfo(win)[1]
  local source_z = vim.api.nvim_win_get_config(win).zindex or 0
  for _, other in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if other ~= win and vim.bo[vim.api.nvim_win_get_buf(other)].filetype == "ipynb_output" then
      local config = vim.api.nvim_win_get_config(other)
      local info = vim.fn.getwininfo(other)[1]
      if config.relative ~= "" and (config.zindex or 50) > source_z and info.winrow < source.winrow + source.height
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

api.from_file = function(path, opts)
  if not ok then return nil end
  opts = opts or {}
  if opts.window == vim.NIL then opts.window = nil end
  local id = opts.id or path
  local img = images[id]
  if not img then
    local loaded, result = pcall(image.from_file, path, opts)
    if not loaded or not result then return nil end
    img = result
    images[id] = img
    local render = img.render
    -- Guard provider-triggered redraws and asynchronous conversion callbacks too.
    img.render = function(self, geometry)
      if self.ipynb_active == false or not self.window or not vim.api.nvim_win_is_valid(self.window) then return end
      local config = vim.api.nvim_win_get_config(self.window)
      if covered_by_output(self.window) then
        self:clear()
        return
      end
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

api.image_size = function(id)
  local img = images[id]
  local size = term and term.get_size()
  if not img or not size or not positive(size.cell_width) or not positive(size.cell_height)
    or not positive(img.image_width) or not positive(img.image_height) then
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
    -- Inline plots share the window with code. Enlarge small source images,
    -- but keep space around them; explicit provider caps still take precedence.
    if img.render_offset_top > 0 then
      width = math.max(1, math.floor(width * 0.8))
      height = math.max(1, math.floor(height * 0.6))
    end
    if positive(opts.max_width_window_percentage) then
      width = math.min(width, math.max(1, math.floor(bounds.width * opts.max_width_window_percentage / 100)))
    end
    if positive(opts.max_height_window_percentage) then
      height = math.min(height, math.max(1, math.floor(bounds.height * opts.max_height_window_percentage / 100)))
    end
    if positive(opts.max_width) then width = math.min(width, opts.max_width) end
    if positive(opts.max_height) then height = math.min(height, opts.max_height) end
  end
  local scale = math.min(
    width * size.cell_width / img.image_width,
    height * size.cell_height / img.image_height
  )
  -- Keep fractional cell geometry for pixel aspect ratio; reserve whole rows.
  -- Avoid ceil(23.000000000000004) reserving a spurious row at the bound.
  img.geometry.width = math.min(width, img.image_width * scale / size.cell_width)
  img.geometry.height = math.min(height, img.image_height * scale / size.cell_height)
  return { width = math.ceil(img.geometry.width), height = math.ceil(img.geometry.height) }
end

api.render = function(id)
  local img = images[id]
  if not img then return end
  if not img.window or not vim.api.nvim_win_is_valid(img.window)
    or vim.api.nvim_win_get_buf(img.window) ~= img.buffer then
    img.window = vim.fn.bufwinid(img.buffer)
  end
  if img.window ~= -1 and vim.api.nvim_win_is_valid(img.window) then
    img.ipynb_active = true
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

api.refresh = function()
  for id, img in pairs(images) do
    if img.ipynb_active then api.render(id) end
  end
end

vim.api.nvim_create_autocmd({ "WinScrolled", "WinClosed", "WinResized" }, {
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
