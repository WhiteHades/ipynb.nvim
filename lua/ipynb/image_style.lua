-- Derived display assets only. Notebook images are never modified.
local M = {}
local directory
local pending, ready, idle, jobs = {}, {}, {}, {}
local closing = false
local IDLE_FILES, IDLE_BYTES = 16, 8 * 1024 * 1024

function M.metrics(size)
  local outer = size.cell_width * 0.5
  local stroke = math.max(1, math.floor(size.cell_width * 0.1 + 0.5))
  -- Tiny images need optical rounding because their padding exceeds the native outer curve.
  return outer, stroke, math.max(outer - size.cell_width - stroke, stroke)
end

local function release(entry)
  local live = true
  return function()
    if not live then return end
    live = false
    entry.refs = entry.refs - 1
    if entry.refs == 0 then
      idle[#idle + 1] = entry
      local bytes = 0
      for _, item in ipairs(idle) do bytes = bytes + item.bytes end
      while #idle > IDLE_FILES or bytes > IDLE_BYTES do
        local old = table.remove(idle, 1)
        ready[old.key] = nil
        vim.fn.delete(old.path)
        bytes = bytes - old.bytes
      end
    end
  end
end

local function acquire(entry)
  for index, item in ipairs(idle) do
    if item == entry then table.remove(idle, index); break end
  end
  entry.refs = entry.refs + 1
  return release(entry)
end

local function deliver(callback, path, done)
  local ok, err = pcall(callback, path, done)
  if not ok then
    if done then done() end
    vim.notify("ipynb.nvim: image style callback failed: " .. tostring(err), vim.log.levels.ERROR)
  end
end

local function failed(key, request, path)
  if pending[key] ~= request then return end
  pending[key] = nil
  if path then vim.fn.delete(path) end
  M.failed = true
  for _, done in ipairs(request.callbacks) do deliver(done, nil) end
  vim.notify("ipynb.nvim: ImageMagick could not style notebook images; using original plots.", vim.log.levels.WARN)
end

function M.generate(key, args, callback)
  if closing then return end
  key = vim.fn.sha256(key)
  if ready[key] then
    deliver(callback, ready[key].path, acquire(ready[key]))
    return
  end
  if pending[key] then pending[key].callbacks[#pending[key].callbacks + 1] = callback; return end
  if not directory then
    directory = vim.fn.tempname() .. "-ipynb-style"
    vim.fn.mkdir(directory, "p", 448)
  end
  local path = directory .. "/" .. key .. ".png"
  local command = { "magick", "-limit", "memory", "64MiB", "-limit", "map", "128MiB" }
  vim.list_extend(command, args)
  command[#command + 1] = path
  local request = { callbacks = { callback } }
  pending[key] = request
  local ok, job = pcall(vim.system, command, { text = true }, vim.schedule_wrap(function(result)
    jobs[request.job] = nil
    if pending[key] ~= request then return end
    if closing then pending[key] = nil; return end
    if result.code ~= 0 then
      failed(key, request, path)
      return
    end
    local stat = vim.uv.fs_stat(path)
    if not stat then
      failed(key, request, path)
      return
    end
    pending[key] = nil
    local entry = { key = key, path = path, bytes = stat.size, refs = #request.callbacks }
    ready[key] = entry
    for _, done in ipairs(request.callbacks) do deliver(done, path, release(entry)) end
  end))
  if not ok or not job then
    failed(key, request, path)
    return
  end
  request.job = job
  jobs[job] = true
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
    local running = {}
    for job in pairs(jobs) do running[#running + 1] = job end
    for _, job in ipairs(running) do pcall(job.kill, job, "sigkill") end
    local confirmed = true
    for _, job in ipairs(running) do
      local ok, result = pcall(job.wait, job, 1000)
      if not ok or not result then confirmed = false end
    end
    if confirmed and directory then vim.fn.delete(directory, "rf") end
  end,
})

return M
