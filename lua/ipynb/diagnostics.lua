local M = {}

function M.attach(buf)
  local keeper = require("otter.keeper")
  local raft = keeper.rafts[buf]
  if not raft or raft.ipynb_diagnostics_attached then return end
  local group = raft.diagnostics_group
  if not group then return end

  for _, autocmd in ipairs(vim.api.nvim_get_autocmds({ group = group, event = "DiagnosticChanged" })) do
    vim.api.nvim_del_autocmd(autocmd.id)
  end
  vim.api.nvim_create_autocmd("DiagnosticChanged", {
    group = group,
    callback = function(args)
      local namespace = raft.diagnostics_namespaces[args.buf]
      if not namespace then return end
      local diagnostics = vim.diagnostic.get(args.buf)
      if OtterConfig.handle_leading_whitespace then
        for _, diagnostic in ipairs(diagnostics) do
          diagnostic.col = diagnostic.col + keeper.get_leading_offset(diagnostic.lnum, buf)
          diagnostic.end_col = diagnostic.end_col
            + keeper.get_leading_offset(diagnostic.end_lnum or diagnostic.lnum, buf)
        end
      end
      vim.diagnostic.set(namespace, buf, diagnostics, {})
    end,
  })
  vim.api.nvim_create_autocmd("LspNotify", {
    group = group,
    callback = function(args)
      if keeper.rafts[buf] ~= raft or not raft.diagnostics_namespaces[args.buf]
        or args.data.method ~= "textDocument/didChange" then return end
      for _, win in ipairs(vim.api.nvim_list_wins()) do
        if vim.api.nvim_win_get_buf(win) == args.buf then return end
      end
      local refresh = vim.lsp.diagnostic and vim.lsp.diagnostic._refresh
      if refresh then refresh(args.buf, args.data.client_id) end
    end,
  })

  local scheduled = false
  vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI", "TextChangedP" }, {
    buffer = buf,
    group = group,
    callback = function()
      if scheduled then return end
      scheduled = true
      vim.schedule(function()
        scheduled = false
        if vim.api.nvim_buf_is_valid(buf) and keeper.rafts[buf] == raft then
          keeper.sync_raft(buf)
        end
      end)
    end,
  })
  raft.ipynb_diagnostics_attached = true
end

return M
