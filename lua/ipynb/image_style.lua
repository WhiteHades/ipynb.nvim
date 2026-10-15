-- Derived display assets only. Notebook images are never modified.
local M = {}
local directory
local pending, ready = {}, {}
local closing = false

function M.metrics(size)
  local radius = size.cell_height * 0.7
  return radius, math.max(1, math.floor(size.cell_width * 0.20 + 0.5)),
    radius * 0.75
end

function M.generate(key, args, callback)
  if closing then return end
  key = vim.fn.sha256(key)
  if ready[key] then callback(ready[key]); return end
  if pending[key] then pending[key][#pending[key] + 1] = callback; return end
  if not directory then
    directory = vim.fn.tempname() .. "-ipynb-style"
    vim.fn.mkdir(directory, "p", 448)
  end
  local path = directory .. "/" .. key .. ".png"
  local command = { "magick", "-limit", "memory", "64MiB", "-limit", "map", "128MiB" }
  vim.list_extend(command, args)
  command[#command + 1] = path
  pending[key] = { callback }
  vim.system(command, { text = true }, vim.schedule_wrap(function(result)
    local callbacks = pending[key] or {}
    pending[key] = nil
    if closing then return end
    if result.code ~= 0 then
      M.failed = true
      local panels = package.loaded["ipynb.panels"]
      if panels then panels.fallback() end
      for _, done in ipairs(callbacks) do done(nil) end
      vim.notify("ipynb.nvim: ImageMagick could not style notebook images; using plain borders and original plots.", vim.log.levels.WARN)
      return
    end
    ready[key] = path
    for _, done in ipairs(callbacks) do done(path) end
  end))
end

function M.rounded(path, width, height, radius, source_width, source_height, callback)
  local scale = math.min(width / source_width, height / source_height)
  local image_width = math.max(1, math.floor(source_width * scale + 0.5))
  local image_height = math.max(1, math.floor(source_height * scale + 0.5))
  radius = math.min(radius, image_width / 2, image_height / 2)
  local size = ("%dx%d"):format(image_width, image_height)
  M.generate(table.concat({ path, size, width, height, radius }, ":"), {
    path .. "[0]", "-resize", size .. "!",
    "(", "-size", size, "xc:none", "-fill", "white", "-draw",
    ("roundrectangle 0,0 %d,%d %g,%g"):format(image_width - 1, image_height - 1, radius, radius), ")",
    "-alpha", "set", "-compose", "DstIn", "-composite",
    "-compose", "Over", "-gravity", "northwest", "-background", "none", "-extent", ("%dx%d"):format(width, height),
  }, callback)
end

vim.api.nvim_create_autocmd("VimLeavePre", {
  group = vim.api.nvim_create_augroup("ipynb_image_style", { clear = true }),
  callback = function()
    closing = true
    if directory then vim.fn.delete(directory, "rf") end
  end,
})

return M
