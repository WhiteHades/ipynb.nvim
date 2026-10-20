local M = {}

local indent_options = {
  "shiftwidth", "tabstop", "softtabstop", "expandtab", "indentexpr", "indentkeys",
  "autoindent", "smartindent", "cindent",
  "comments", "commentstring", "formatoptions",
}
local markdown_query

local function options(buf)
  local result = {}
  for _, name in ipairs(indent_options) do
    result[name] = vim.api.nvim_get_option_value(name, { buf = buf })
  end
  return result
end

function M.python_options(buf)
  local keeper = package.loaded["otter.keeper"]
  local raft = keeper and keeper.rafts[buf]
  local python = raft and raft.buffers.python
  if python and vim.api.nvim_buf_is_valid(python) then return options(python) end
  return options(buf)
end

local function python_ranges(buf)
  local parser = vim.treesitter.get_parser(buf, "markdown")
  local tree = parser:parse()[1]
  markdown_query = markdown_query or vim.treesitter.query.parse("markdown", "(fenced_code_block) @cell")
  local ranges = {}
  for _, node in markdown_query:iter_captures(tree:root(), buf) do
    local info, content
    for child in node:iter_children() do
      if child:type() == "info_string" then info = vim.treesitter.get_node_text(child, buf)
      elseif child:type() == "code_fence_content" then content = child end
    end
    local language = info and info:match("^%s*([%w_+-]+)")
    language = language and language:lower()
    if content and language and (language == "py" or language:match("^python%d*$") or language:match("^py%d+$")) then
      local first, _, last = content:range()
      ranges[#ranges + 1] = { first, last }
    end
  end
  return ranges
end

local function contains(ranges, row)
  for _, range in ipairs(ranges) do
    if row >= range[1] and row < range[2] then return true end
  end
  return false
end

function M.attach(buf)
  if not vim.api.nvim_buf_is_valid(buf) or not vim.b[buf].notebook_markdown
    or vim.b[buf].ipynb_python_indent_attached then return end
  local group = vim.api.nvim_create_augroup("ipynb_python_indent_" .. buf, { clear = true })
  local markdown_options = options(buf)
  local current_tick = -1
  local ranges = {}
  local refresh_pending = false
  local using_python = false
  local update

  local function refresh()
    if not vim.api.nvim_buf_is_valid(buf) then return end
    local tick = vim.api.nvim_buf_get_changedtick(buf)
    local edit = vim.b[buf].ipynb_code_edit
    if not edit or edit.tick ~= tick or current_tick < edit.from_tick then ranges = python_ranges(buf) end
    current_tick = tick
  end

  local function refresh_later()
    if refresh_pending then return end
    refresh_pending = true
    vim.schedule(function()
      refresh_pending = false
      if vim.api.nvim_buf_is_valid(buf) then
        refresh()
        if vim.api.nvim_get_current_buf() == buf then update() end
      end
    end)
  end

  update = function(event)
    if vim.api.nvim_get_current_buf() ~= buf then return end
    if current_tick < 0 then refresh() end
    local row = vim.api.nvim_win_get_cursor(0)[1] - 1
    local stale = current_tick ~= vim.api.nvim_buf_get_changedtick(buf)
    local in_python = contains(ranges, row)
    if stale then
      if not (in_python and event and event.event == "CursorMovedI") then
        refresh()
        in_python = contains(ranges, row)
      end
    end
    if in_python == using_python then return end
    if in_python then
      markdown_options = options(buf)
      local py = M.python_options(buf)
      for _, name in ipairs(indent_options) do
        vim.api.nvim_set_option_value(name, py[name], { buf = buf })
      end
    else
      for _, name in ipairs(indent_options) do
        vim.api.nvim_set_option_value(name, markdown_options[name], { buf = buf })
      end
    end
    using_python = in_python
  end

  vim.api.nvim_create_autocmd({ "CursorMoved", "CursorMovedI", "InsertEnter", "BufEnter", "WinEnter" }, {
    group = group, buffer = buf, callback = update,
  })
  vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI" }, {
    group = group, buffer = buf, callback = refresh_later,
  })
  vim.api.nvim_create_autocmd("BufWipeout", {
    group = group, buffer = buf, once = true, callback = function()
      pcall(vim.api.nvim_del_augroup_by_id, group)
    end,
  })

  vim.b[buf].ipynb_python_indent_attached = true
  refresh()
  update()
end

return M
