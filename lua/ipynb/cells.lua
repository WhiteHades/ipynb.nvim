local M = {}

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
