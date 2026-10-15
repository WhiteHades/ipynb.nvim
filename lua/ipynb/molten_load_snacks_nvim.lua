-- loads the snacks.nvim plugin and exposes methods to the python remote plugin
local ok, snacks = pcall(require, "snacks")

if not ok then
  vim.api.nvim_echo({ { "[Ipynb] `snacks.nvim` not found" } }, true, { err = true })
  return
end

local snacks_api = {}
local images = {}

local function place(img)
  img.placement = Snacks.image.placement.new(img.buffer, img.path, img.opts)
  local image = img.placement.img
  if img.styled and image and not image._ipynb_guard_on_ready then
    local on_ready = image.on_ready
    image.on_ready = function(self, ...)
      if next(self.placements) then return on_ready(self, ...) end
    end
    image._ipynb_guard_on_ready = true
  end
end

local function retire(img)
  local placement = img.placement
  if placement then
    placement:close()
    img.placement = nil
  end
  local release = img.release
  img.release = nil
  if release then
    local convert = placement and placement.img and placement.img._convert
    if convert and not convert:done() then
      local on_done = convert.opts.on_done
      convert.opts.on_done = function(...)
        local ok, err = pcall(on_done, ...)
        vim.schedule(release)
        if not ok then error(err, 0) end
      end
    else
      release()
    end
  end
end

snacks_api.from_file = function(path, opts)
  local identifier = opts.id or path
  local previous = images[identifier]
  if previous then retire(previous) end
  local doc = snacks.config.image.doc or {}
  local max_width = opts.max_width or doc.max_width or 80
  local max_height = opts.max_height or doc.max_height or 40
  local size = snacks.image.util.fit(path, { width = max_width, height = max_height })
  local row, col = (opts.y or 0) + 1, opts.x or 0
  local placement_opts = {
    inline = true,
    conceal = true,
    pos = { row, col },
    range = { row, col, row + size.height - 1, col + size.width },
    width = size.width,
    height = size.height,
    auto_resize = true,
    max_width = size.width,
    max_height = size.height,
  }

  local img = { path = path, size = size, buffer = opts.buffer, opts = placement_opts }
  images[identifier] = img
  local pixels = snacks.image.util.dim(path)
  local terminal = snacks.image.terminal.size()
  local style = require("ipynb.image_style")
  local _, _, radius = style.metrics(terminal)
  style.rounded(path,
    math.floor(size.width * terminal.cell_width + 0.5),
    math.floor(size.height * terminal.cell_height + 0.5), radius, pixels.width, pixels.height, function(styled, release)
      if images[identifier] ~= img then if release then release() end; return end
      img.release = release
      if styled then img.path, img.styled = styled, true end
      img.ready = true
      if img.render_requested and not img.placement then
        place(img)
      end
    end)
  return identifier
end

snacks_api.render = function(identifier)
  local img = images[identifier]

  if img and img.placement == nil then
    img.render_requested = true
    if img.ready then
      place(img)
    end
  end
end

snacks_api.clear = function(identifier)
  local img = images[identifier]
  if img then retire(img) end
  images[identifier] = nil
end

snacks_api.clear_all = function()
  for identifier in pairs(images) do
    snacks_api.clear(identifier)
  end
end

--- try to estimate actual size based on raw image and snack opts. Actual rendered size is somewhat random but we cannot get it due to it not being available until after render()
snacks_api.image_size = function(identifier)
  local img = images[identifier]
  if not img then return { width = 0, height = 0 } end
  return img.size
end

return { snacks_api = snacks_api }
