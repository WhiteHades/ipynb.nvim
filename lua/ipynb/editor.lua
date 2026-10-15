local M = {}

function M.format(buf)
  local spans = require("ipynb.cells").executable(buf)
  local raft = require("otter.keeper").rafts[buf]
  local edits = {}
  for index = #spans, 1, -1 do
    local span = spans[index]
    local fence = vim.api.nvim_buf_get_lines(buf, span.begin[1] - 1, span.begin[1], false)[1] or ""
    local language = fence:match("^```%s*([%w_+-]+)")
    local extension = language and OtterConfig.extensions[language]
    if extension then
      local original = vim.api.nvim_buf_get_lines(buf, span.begin[1], span["end"][1] + 1, false)
      local scratch = vim.api.nvim_create_buf(false, true)
      vim.api.nvim_create_autocmd("BufWriteCmd", { buffer = scratch, callback = function() end })
      local ok, formatted = xpcall(function()
        vim.api.nvim_buf_set_name(scratch, vim.api.nvim_buf_get_name(buf) .. ".cell-" .. index .. "." .. extension)
        vim.bo[scratch].buftype = ""
        vim.api.nvim_buf_set_lines(scratch, 0, -1, false, original)
        vim.bo[scratch].filetype = vim.filetype.match({ filename = vim.api.nvim_buf_get_name(scratch) }) or language
        vim.b[scratch].autoformat = vim.b[buf].autoformat
        vim.b[scratch].disable_autoformat = vim.b[buf].disable_autoformat
        local embedded = raft and raft.buffers[language]
        for _, client in ipairs(embedded and vim.lsp.get_clients({ bufnr = embedded }) or {}) do
          if client:supports_method("textDocument/formatting", embedded) then
            vim.lsp.buf_attach_client(scratch, client.id)
          end
        end
        vim.api.nvim_buf_call(scratch, function()
          vim.api.nvim_exec_autocmds("BufWritePre", { buffer = scratch, modeline = false })
        end)
        return vim.api.nvim_buf_get_lines(scratch, 0, -1, false)
      end, debug.traceback)
      vim.api.nvim_buf_delete(scratch, { force = true })
      if not ok then error(formatted) end
      if not vim.deep_equal(original, formatted) then edits[#edits + 1] = { span = span, lines = formatted } end
    end
  end
  -- Apply bottom to top so formatter line changes cannot move another cell's fences.
  for _, edit in ipairs(edits) do
    vim.api.nvim_buf_set_lines(buf, edit.span.begin[1], edit.span["end"][1] + 1, false, edit.lines)
  end
end

function M.lint(buf)
  require("otter").sync_raft(buf)
  local raft = require("otter.keeper").rafts[buf]
  local lint = package.loaded.lint
  if not raft or not lint then return end
  local on_disk = OtterConfig.buffers.write_to_disk
  for _, embedded in pairs(raft.buffers) do
    vim.api.nvim_buf_call(embedded, function()
      if on_disk then vim.cmd("silent noautocmd write") end
      local names = vim.list_extend({}, lint._resolve_linter_by_ft(vim.bo[embedded].filetype))
      if #names == 0 then vim.list_extend(names, lint.linters_by_ft._ or {}) end
      vim.list_extend(names, lint.linters_by_ft["*"] or {})
      local filename = vim.api.nvim_buf_get_name(embedded)
      local context = { filename = filename, dirname = vim.fn.fnamemodify(filename, ":h") }
      lint.try_lint(names, { filter = function(linter)
        return (linter.stdin == true or on_disk) and (not linter.condition or linter.condition(context))
      end })
    end)
  end
end

return M
