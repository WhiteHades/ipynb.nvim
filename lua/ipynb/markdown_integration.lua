local M = {}
local installed, reported = false, false
local cache = {}
local query
local languages = { "html", "latex", "markdown", "markdown_inline", "yaml" }

local function unavailable(reason)
  installed = false
  if reported then return end
  reported = true
  vim.schedule(function()
    vim.notify("Notebook Markdown integration unavailable; using ipynb prose rendering: " .. tostring(reason),
      vim.log.levels.WARN, { title = "ipynb" })
  end)
end

function M.active()
  return installed
end

local function excluded(bufnr)
  local tick = vim.api.nvim_buf_get_changedtick(bufnr)
  if cache[bufnr] and cache[bufnr].tick == tick then return cache[bufnr].ranges end

  local ranges = {}
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  if lines[1] == "---" and lines[2] == "jupyter:" then
    for index = 3, #lines do
      if lines[index] == "---" then ranges[#ranges + 1] = { 0, index }; break end
    end
  end

  local parser = vim.treesitter.get_parser(bufnr, "markdown")
  local tree = assert(parser:parse()[1], "missing Markdown tree")
  query = query or vim.treesitter.query.parse("markdown", "(fenced_code_block) @cell")
  for _, node in query:iter_captures(tree:root(), bufnr) do
    local first, _, last, col = node:range()
    ranges[#ranges + 1] = { first, last + (col > 0 and 1 or 0) }
  end
  cache[bufnr] = { tick = tick, ranges = ranges }
  return ranges
end

local function overlaps(mark, ranges)
  local last = math.max(mark.start_row + 1,
    (mark.opts.end_row or mark.start_row) + ((mark.opts.end_col or 0) > 0 and 1 or 0))
  for _, range in ipairs(ranges) do
    if mark.start_row < range[2] and last > range[1] then return true end
  end
  return false
end

local function wrap(builtin, previous)
  local function parse(ctx)
    if not previous then return builtin.parse(ctx) end
    local custom = previous.parse(ctx)
    if not previous.extends then return custom end
    local marks = {}
    vim.list_extend(marks, custom)
    vim.list_extend(marks, builtin.parse(ctx))
    return marks
  end

  return { extends = false, parse = function(ctx)
    if not vim.api.nvim_buf_get_name(ctx.buf):match("%.ipynb$") then return parse(ctx) end
    if not installed then return {} end
    local ok, marks = pcall(function()
      local ranges = excluded(ctx.buf)
      local result = {}
      for _, mark in ipairs(parse(ctx)) do
        if not overlaps(mark, ranges) then result[#result + 1] = mark end
      end
      return result
    end)
    if ok then return marks end
    unavailable(marks)
    return {}
  end }
end

function M.setup(opts)
  local handlers = {}
  -- Verified against render-markdown 8.13.0; unavailable builtin parse paths keep the fallback active.
  for _, language in ipairs(languages) do
    local ok, builtin = pcall(require, "render-markdown.handler." .. language)
    local previous = (opts.custom_handlers or {})[language]
    if not ok or type(builtin) ~= "table" or type(builtin.parse) ~= "function" then
      unavailable("missing render-markdown.handler." .. language .. ".parse")
      return
    end
    if previous and (type(previous) ~= "table" or type(previous.parse) ~= "function") then
      unavailable("invalid custom handler for " .. language)
      return
    end
    handlers[language] = wrap(builtin, previous)
  end
  opts.custom_handlers = vim.tbl_extend("force", opts.custom_handlers or {}, handlers)
  if not installed then
    vim.api.nvim_create_autocmd("BufWipeout", { callback = function(ev) cache[ev.buf] = nil end })
  end
  installed = true
end

return M
