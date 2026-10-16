local M = {}

local function hidden_window(buf)
  -- Save hooks can yield and redraw; an autocmd window can then cover the notebook.
  local win = vim.api.nvim_open_win(buf, false, {
    relative = "editor", row = 0, col = 0,
    width = vim.api.nvim_win_get_width(0), height = vim.api.nvim_win_get_height(0),
    hide = true, noautocmd = true, focusable = false, style = "minimal",
  })
  vim.wo[win].statusline = vim.wo.statusline
  vim.w[win].ipynb_editor = true
  return win
end

local function close_window(win)
  if win and vim.api.nvim_win_is_valid(win) then
    vim.cmd(("noautocmd call nvim_win_close(%d, v:true)"):format(win))
  end
end

local function hidden_buffer_call(buf, callback)
  local win = hidden_window(buf)
  local ok, result = xpcall(function() return vim.api.nvim_win_call(win, callback) end, debug.traceback)
  close_window(win)
  if not ok then error(result, 0) end
  return result
end

local function stateful_hooks(buf)
  for _, hook in ipairs(vim.api.nvim_get_autocmds({ event = "BufWritePre", buffer = buf })) do
    if hook.group_name ~= ("nvim.lsp.b_%d_save"):format(buf)
      or hook.desc ~= "vim.lsp: textDocument/willSave" then return true end
    for _, client in ipairs(vim.lsp.get_clients({ bufnr = buf })) do
      if client:supports_method("textDocument/willSave")
        or client:supports_method("textDocument/willSaveWaitUntil") then return true end
    end
  end
  return false
end

function M.format(buf)
  local spans = require("ipynb.cells").executable(buf)
  local raft = require("otter.keeper").rafts[buf]
  local edits, contexts = {}, {}
  local ok, err = xpcall(function()
    for index = #spans, 1, -1 do
      local span = spans[index]
      local fence = vim.api.nvim_buf_get_lines(buf, span.begin[1] - 1, span.begin[1], false)[1] or ""
      local delimiter, language = fence:match("^(```+)%s*([%w_+-]+)")
      local extension = language and OtterConfig.extensions[language]
      if extension then
        local finish = span["end"][1] + 1
        local original = vim.api.nvim_buf_get_lines(buf, span.begin[1], finish, false)
        local closing = #original == 1 and original[1]:match("^(```+)%s*$")
        if closing and #closing >= #delimiter then
          original, finish = {}, span.begin[1]
        end
        local context = contexts[language]
        local fresh = not context
        if fresh then
          context = { buf = vim.api.nvim_create_buf(false, true) }
          contexts[language] = context
          vim.api.nvim_create_autocmd("BufWriteCmd", { buffer = context.buf, callback = function() end })
          context.win = hidden_window(context.buf)
        end
        local scratch = context.buf
        local formatted = vim.api.nvim_win_call(context.win, function()
          vim.api.nvim_buf_set_lines(scratch, 0, -1, false, original)
          vim.api.nvim_win_set_cursor(context.win, { 1, 0 })
          if fresh then
            vim.api.nvim_buf_set_name(scratch, vim.api.nvim_buf_get_name(buf) .. ".cell-" .. index .. "." .. extension)
            vim.bo[scratch].buftype = ""
            vim.bo[scratch].filetype = vim.filetype.match({ filename = vim.api.nvim_buf_get_name(scratch) }) or language
            vim.b[scratch].autoformat = vim.b[buf].autoformat
            vim.b[scratch].disable_autoformat = vim.b[buf].disable_autoformat
            local embedded = raft and raft.buffers[language]
            for _, client in ipairs(embedded and vim.lsp.get_clients({ bufnr = embedded }) or {}) do
              if client:supports_method("textDocument/formatting", embedded) then
                vim.lsp.buf_attach_client(scratch, client.id)
              end
            end
          end
          local variables = vim.fn.getbufvar(scratch, "")
          variables.changedtick = nil
          local local_hooks = stateful_hooks(scratch)
          vim.api.nvim_exec_autocmds("BufWritePre", { buffer = scratch, modeline = false })
          local after = vim.fn.getbufvar(scratch, "")
          after.changedtick = nil
          -- Stateful save hooks need the same fresh cell context as a separate file.
          context.reusable = not local_hooks
            and not stateful_hooks(scratch)
            and vim.deep_equal(variables, after)
          return vim.api.nvim_buf_get_lines(scratch, 0, -1, false)
        end)
        if #original == 0 and #formatted == 1 and formatted[1] == "" then formatted = {} end
        if not vim.deep_equal(original, formatted) then
          edits[#edits + 1] = { span = span, finish = finish, lines = formatted }
        end
        if not context.reusable then
          close_window(context.win)
          vim.api.nvim_buf_delete(scratch, { force = true })
          contexts[language] = nil
        end
      end
    end
  end, debug.traceback)
  for _, context in pairs(contexts) do
    close_window(context.win)
    if vim.api.nvim_buf_is_valid(context.buf) then vim.api.nvim_buf_delete(context.buf, { force = true }) end
  end
  if not ok then error(err, 0) end
  -- Apply bottom to top so formatter line changes cannot move another cell's fences.
  for _, edit in ipairs(edits) do
    vim.api.nvim_buf_set_lines(buf, edit.span.begin[1], edit.finish, false, edit.lines)
  end
end

function M.lint(buf)
  require("otter").sync_raft(buf)
  local raft = require("otter.keeper").rafts[buf]
  local lint = package.loaded.lint
  if not raft or not lint then return end
  local on_disk = OtterConfig.buffers.write_to_disk
  for _, embedded in pairs(raft.buffers) do
    hidden_buffer_call(embedded, function()
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
