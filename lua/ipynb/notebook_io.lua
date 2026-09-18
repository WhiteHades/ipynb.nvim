local M = {}

local state

local function local_notebook(path)
  return type(path) == "string" and path:sub(-6) == ".ipynb" and not path:find("://", 1, true)
end

local function option(jupytext, name)
  return jupytext.get_option(name)
end

local function registered(name)
  return vim.fn.exists("*" .. name) == 1 or vim.fn.exists("#FuncUndefined#" .. name) == 1
end

local function fast_path(jupytext, path)
  return local_notebook(path)
    and registered("IpynbNotebookRead")
    and registered("IpynbNotebookWrite")
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
  local result = vim.fn.IpynbNotebookRead(path, option(jupytext, "new_template"))
  local text = result.text or ""
  local lines = vim.split(text, "\n", { plain = true })
  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
  -- Kernel selection follows BufReadPost; reuse metadata already decoded here.
  vim.b[bufnr].ipynb_read_kernel = ((result.metadata or {}).kernelspec or {}).name or false
  set_filetype(jupytext, path, result.metadata or {}, bufnr)
  vim.api.nvim_set_option_value("modified", false, { buf = bufnr })
  return result.metadata
end

local function write_notebook(jupytext, path, _metadata, bufnr)
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

return M
