local M = {}

function M.spans(bufnr)
  local parser = vim.treesitter.get_parser(bufnr, "markdown")
  local tree = parser:parse()[1]
  local query = vim.treesitter.query.parse("markdown", "(fenced_code_block) @cell")
  local result = {}
  for _, node in query:iter_captures(tree:root(), bufnr) do
    local first, _, last, col = node:range()
    local start, finish = first + 1, last - (col == 0 and 2 or 1)
    if start <= finish then
      local line = vim.api.nvim_buf_get_lines(bufnr, finish, finish + 1, false)[1] or ""
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
  if not force and vim.b[bufnr].ipynb_code_spans_tick == tick then return vim.b[bufnr].ipynb_code_spans end
  local spans = require("ipynb.native").request("code_spans", {
    path = vim.api.nvim_buf_get_name(bufnr), spans = M.spans(bufnr),
    text = table.concat(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false), "\n"),
  })
  vim.b[bufnr].ipynb_code_spans, vim.b[bufnr].ipynb_code_spans_tick = spans, tick
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
