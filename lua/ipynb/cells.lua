local M = {}
local cache = {}

local function cache_spans(bufnr, spans, tick)
  if not cache[bufnr] then
    cache[bufnr] = {}
    vim.api.nvim_buf_attach(bufnr, false, {
      on_lines = function(_, changed_buf, changed_tick, first, last, new_last)
        local state = cache[changed_buf]
        local spans = state.spans
        state.spans = nil
        local previous = vim.b[changed_buf].ipynb_code_edit
        vim.b[changed_buf].ipynb_code_edit = nil
        if not spans or changed_tick ~= state.tick + 1 then return end
        for _, line in ipairs(vim.api.nvim_buf_get_lines(changed_buf, first, new_last, false)) do
          if line:match("^%s*[`~]") or line:find("<!--", 1, true) or line:find("-->", 1, true)
            or line:match("#region") or line:match("#endregion") then return end
        end
        for index, span in ipairs(spans) do
          local finish = span["end"][1] - (state.bodyless[index] and 1 or 0)
          if first >= span.begin[1] and last <= finish + 1 then
            local delta = new_last - last
            finish = finish + delta
            state.bodyless[index] = finish < span.begin[1]
            local line = not state.bodyless[index]
              and vim.api.nvim_buf_get_lines(changed_buf, finish, finish + 1, false)[1] or ""
            span["end"] = { math.max(span.begin[1], finish), #line }
            for following = index + 1, #spans do
              spans[following].begin[1] = spans[following].begin[1] + delta
              spans[following]["end"][1] = spans[following]["end"][1] + delta
            end
            if last == first + 1 and new_last == last then
              local from_tick = previous and previous.tick == state.tick and previous.row == first
                and previous.from_tick or state.tick
              local opening = vim.api.nvim_buf_get_lines(changed_buf, span.begin[1] - 1, span.begin[1], false)[1] or ""
              vim.b[changed_buf].ipynb_code_edit = {
                from_tick = from_tick, tick = changed_tick, row = first,
                bordered = opening:match("^```+%s*[%w_+-]+") ~= nil,
              }
            end
            state.spans, state.tick = spans, changed_tick
            vim.b[changed_buf].ipynb_code_spans = spans
            vim.b[changed_buf].ipynb_code_spans_tick = changed_tick
            return
          end
        end
      end,
      on_detach = function(_, detached_buf)
        cache[detached_buf] = nil
      end,
    })
  end
  cache[bufnr].spans, cache[bufnr].tick = vim.deepcopy(spans), tick
  cache[bufnr].bodyless = {}
  for index, span in ipairs(spans) do
    local line = vim.api.nvim_buf_get_lines(bufnr, span["end"][1], span["end"][1] + 1, false)[1] or ""
    cache[bufnr].bodyless[index] = span.begin[1] == span["end"][1] and span["end"][2] == 0
      and line:match("^%s*[`~]") ~= nil
  end
end

function M.spans(bufnr)
  local parser = vim.treesitter.get_parser(bufnr, "markdown")
  local tree = parser:parse()[1]
  local query = vim.treesitter.query.parse("markdown", "(fenced_code_block) @cell")
  local result = {}
  for _, node in query:iter_captures(tree:root(), bufnr) do
    local first, _, last, col = node:range()
    local start, finish = first + 1, last - (col == 0 and 2 or 1)
    local closing = node:child(node:child_count() - 1)
    local empty = closing and closing:type() == "fenced_code_block_delimiter" and closing:start() == start
    if empty then finish = start end
    if start <= finish then
      local line = not empty and vim.api.nvim_buf_get_lines(bufnr, finish, finish + 1, false)[1] or ""
      result[#result + 1] = { begin = { start, 0 }, ["end"] = { finish, #line } }
    end
  end
  return result
end

function M.executable(bufnr, force)
  local tick = vim.api.nvim_buf_get_changedtick(bufnr)
  if force or vim.b[bufnr].ipynb_code_spans_tick ~= tick then
    local read = vim.b[bufnr].ipynb_code_spans_read
    vim.b[bufnr].ipynb_code_spans_read = nil
    -- BufReadCmd advances changedtick after returning, even when the text is unchanged.
    if not force and read and vim.deep_equal(read, vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)) then
      vim.b[bufnr].ipynb_code_spans_tick = tick
    end
  end
  if not force and vim.b[bufnr].ipynb_code_spans_tick == tick then
    local spans = vim.b[bufnr].ipynb_code_spans
    if not cache[bufnr] or cache[bufnr].tick ~= tick or not cache[bufnr].spans then cache_spans(bufnr, spans, tick) end
    return spans
  end
  local spans = require("ipynb.native").request("code_spans", {
    path = vim.api.nvim_buf_get_name(bufnr), spans = M.spans(bufnr),
    text = table.concat(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false), "\n"),
  })
  vim.b[bufnr].ipynb_code_spans, vim.b[bufnr].ipynb_code_spans_tick = spans, tick
  cache_spans(bufnr, spans, tick)
  return spans
end

local function cells()
  local parser = vim.treesitter.get_parser(0, "markdown")
  local tree = parser:parse()[1]
  local query = vim.treesitter.query.parse("markdown", "(fenced_code_block) @cell")
  local result = {}
  for _, node in query:iter_captures(tree:root(), 0) do
    local first, _, last, col = node:range()
    if col > 0 then last = last + 1 end
    result[#result + 1] = { first = first, last = last }
  end
  return result
end

function M.current()
  local row = vim.api.nvim_win_get_cursor(0)[1] - 1
  for _, cell in ipairs(cells()) do
    if row >= cell.first and row < cell.last then return cell end
  end
end

function M.add(kind)
  local cell = M.current()
  local row = cell and cell.last or vim.api.nvim_win_get_cursor(0)[1]
  local lines = kind == "markdown" and { "", "<!-- #region -->", "", "<!-- #endregion -->", "" }
    or { "", "```python", "", "```", "" }
  vim.api.nvim_buf_set_lines(0, row, row, false, lines)
  vim.api.nvim_win_set_cursor(0, { row + 3, 0 })
  vim.cmd("doautocmd TextChanged")
end

function M.advance()
  local current = M.current()
  if not current then return end
  for _, cell in ipairs(cells()) do
    if cell.first >= current.last then
      vim.api.nvim_win_set_cursor(0, { cell.first + 2, 0 })
      return
    end
  end
  M.add("code")
end

return M
