local M = {}

local state

local function local_notebook(path)
  return type(path) == "string" and path:sub(-6) == ".ipynb" and not path:find("://", 1, true)
end

local function option(jupytext, name)
  return jupytext.get_option(name)
end

local function fast_path(jupytext, path)
  return local_notebook(path)
    and option(jupytext, "format") == "md:markdown"
    and option(jupytext, "autosync") == false
    and option(jupytext, "async_write") == false
    and option(jupytext, "update") == true
end

local function absolute_or_real(path)
  return vim.uv.fs_realpath(path) or vim.fn.fnamemodify(path, ":p")
end

local function same_file(left, right)
  return left ~= "" and absolute_or_real(left) == absolute_or_real(right)
end

local function write_in_place(path, bufnr)
  return same_file(vim.api.nvim_buf_get_name(bufnr), path)
end

local function set_filetype(jupytext, path, metadata, bufnr)
  local filetype = option(jupytext, "filetype")
  if type(filetype) == "function" then
    filetype = filetype(vim.uv.fs_realpath(path), "md:markdown", metadata)
  end
  vim.api.nvim_set_option_value("filetype", filetype, { buf = bufnr })
end

local function open_notebook(jupytext, path, bufnr)
  path = vim.fn.fnamemodify(path, ":p")
  if bufnr == 0 then bufnr = vim.api.nvim_get_current_buf() end
  vim.b[bufnr].ipynb_code_spans_read = nil
  -- :edit! replaces the buffer before the converter replies. Old extmarks
  -- would otherwise collapse onto EOF and be reused for the reloaded cells.
  local ui = package.loaded["ipynb.native_ui"]
  if ui then ui.clear(bufnr) end
  local result = vim.fn.IpynbNotebookRead(path, option(jupytext, "new_template"))
  local text = result.text or ""
  local lines = vim.split(text, "\n", { plain = true })
  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
  -- Kernel selection follows BufReadPost; reuse metadata already decoded here.
  vim.b[bufnr].ipynb_read_kernel = ((result.metadata or {}).kernelspec or {}).name or false
  vim.b[bufnr].ipynb_language = ((result.metadata or {}).language_info or {}).name
    or ((result.metadata or {}).kernelspec or {}).language
  if type(result.paired_changed) == "table" and #result.paired_changed > 0 then
    vim.schedule(function()
      vim.notify("Jupytext pairs differ. Use :IpynbJupytext sync to choose the source before saving.", vim.log.levels.WARN)
    end)
  end
  if vim.fn.filereadable(path) == 1 then
    local tick = vim.api.nvim_buf_get_changedtick(bufnr)
    local spans = require("ipynb.native").request("preview", { buf = bufnr, path = path, lines = lines,
      spans = require("ipynb.cells").spans(bufnr) })
    vim.b[bufnr].ipynb_code_spans, vim.b[bufnr].ipynb_code_spans_tick = spans, tick
    vim.b[bufnr].ipynb_code_spans_read = lines
  end
  set_filetype(jupytext, path, result.metadata or {}, bufnr)
  vim.api.nvim_set_option_value("modified", false, { buf = bufnr })
  return result.metadata
end

local function write_notebook(jupytext, path, _metadata, bufnr)
  path = vim.fn.fnamemodify(path, ":p")
  local in_place = write_in_place(path, bufnr)
  local expected_mtime = in_place and vim.b[bufnr].mtime or vim.NIL
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local result = vim.fn.IpynbNotebookWrite(path, table.concat(lines, "\n"), expected_mtime)
  if in_place or vim.o.cpoptions:find("%+") ~= nil then
    vim.api.nvim_set_option_value("modified", false, { buf = bufnr })
  end
  if in_place and result and result.mtime then
    vim.b[bufnr].mtime = result.mtime
  end
  return result
end

function M.setup(jupytext)
  if state and state.jupytext == jupytext then return end

  local original_open = jupytext.open_notebook
  local original_write = jupytext.write_notebook
  state = { jupytext = jupytext }
  jupytext.open_notebook = function(path, bufnr)
    if fast_path(jupytext, path) then
      return open_notebook(jupytext, path, bufnr or 0)
    end
    vim.b[bufnr or 0].ipynb_read_kernel = nil
    return original_open(path, bufnr)
  end
  jupytext.write_notebook = function(path, metadata, bufnr)
    if fast_path(jupytext, path) then
      return write_notebook(jupytext, path, metadata, bufnr or 0)
    end
    return original_write(path, metadata, bufnr)
  end
end

function M.attach(buf)
  local path = vim.api.nvim_buf_get_name(buf)
  if not state or not fast_path(state.jupytext, path) then return end
  vim.bo[buf].buftype = "acwrite"
  local group = vim.api.nvim_create_augroup("JupytextPlugin" .. buf, { clear = false })
  if #vim.api.nvim_get_autocmds({ event = "BufWriteCmd", buffer = buf, group = group }) == 0 then return end
  vim.api.nvim_clear_autocmds({ event = "BufWriteCmd", buffer = buf, group = group })
  vim.api.nvim_create_autocmd("BufWriteCmd", {
    buffer = buf, group = group, nested = true,
    callback = function(event)
      vim.api.nvim_buf_call(buf, function()
        vim.api.nvim_exec_autocmds("BufWritePre", { pattern = event.file, modeline = false })
        write_notebook(state.jupytext, event.file, nil, buf)
        vim.api.nvim_exec_autocmds("BufWritePost", { pattern = event.file, modeline = false })
      end)
    end,
  })
  vim.bo[buf].buftype = ""
end

return M
