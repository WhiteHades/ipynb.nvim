local M = {}

local state
local writing = {}

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

function M.check_read(bufnr)
  if vim.b[bufnr].ipynb_read_incomplete then
    error("Notebook did not finish loading; reload successfully before saving", 0)
  end
end

local function set_filetype(jupytext, path, metadata, bufnr, format)
  local filetype = option(jupytext, "filetype")
  if type(filetype) == "function" then
    filetype = filetype(vim.uv.fs_realpath(path), format or "md:markdown", metadata)
  end
  vim.api.nvim_set_option_value("filetype", filetype, { buf = bufnr })
end

local function jupytext_command(command, input)
  local result = vim.system(command, { text = true, stdin = input }):wait()
  if result.code ~= 0 then
    error("Jupytext command failed with status " .. result.code .. ": " .. (result.stderr or ""), 0)
  end
  return result.stdout
end

local function open_fallback(jupytext, path, bufnr)
  local source = vim.uv.fs_realpath(path)
  if source and option(jupytext, "autosync") then
    jupytext_command({ option(jupytext, "jupytext"), "--sync", source })
  end
  local lines = jupytext.read_file(source or option(jupytext, "new_template"), true)
  local metadata = jupytext.get_metadata(lines)
  local format = option(jupytext, "format")
  if type(format) == "function" then format = format(source, metadata) end
  if format ~= "ipynb" then
    local text = jupytext_command({ option(jupytext, "jupytext"), "--from", "ipynb",
      "--to", format, "--output", "-" }, lines)
    lines = vim.split(text:gsub("\n$", ""), "\n", { plain = true })
  end
  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
  set_filetype(jupytext, path, metadata, bufnr, format)
  vim.bo[bufnr].modified = false
  return metadata
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
  if bufnr == 0 then bufnr = vim.api.nvim_get_current_buf() end
  M.check_read(bufnr)
  path = vim.fn.fnamemodify(path, ":p")
  local in_place = write_in_place(path, bufnr)
  local expected_mtime = in_place and vim.b[bufnr].mtime or vim.NIL
  local name = vim.api.nvim_buf_get_name(bufnr)
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local tick = vim.api.nvim_buf_get_changedtick(bufnr)
  local result = vim.api.nvim_buf_call(bufnr, function()
    return vim.fn.IpynbNotebookWrite(path, table.concat(lines, "\n"), expected_mtime)
  end)
  if vim.api.nvim_buf_get_name(bufnr) == name and vim.api.nvim_buf_get_changedtick(bufnr) == tick
    and (in_place or vim.o.cpoptions:find("%+") ~= nil) then
    vim.api.nvim_set_option_value("modified", false, { buf = bufnr })
  end
  if in_place and write_in_place(path, bufnr) and result and result.mtime then
    vim.b[bufnr].mtime = result.mtime
  end
  return result
end

function M.setup(jupytext)
  if state and state.jupytext == jupytext then return end

  local original_write = jupytext.write_notebook
  state = { jupytext = jupytext }
  jupytext.open_notebook = function(path, bufnr)
    if not bufnr or bufnr == 0 then bufnr = vim.api.nvim_get_current_buf() end
    vim.b[bufnr].ipynb_read_incomplete = true
    local result
    if fast_path(jupytext, path) then
      result = open_notebook(jupytext, path, bufnr)
    else
      vim.b[bufnr].ipynb_read_kernel = nil
      result = open_fallback(jupytext, path, bufnr)
    end
    vim.b[bufnr].ipynb_read_incomplete = nil
    return result
  end
  jupytext.write_notebook = function(path, metadata, bufnr)
    M.check_read(bufnr or 0)
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
      M.check_read(buf)
      if writing[buf] then
        if not same_file(writing[buf].path, event.file) then error("Notebook save in progress; retry the other destination", 0) end
        writing[buf].again = true
        return
      end
      -- Engine waits process events; another save must not reuse the in-flight disk token.
      local active = { path = event.file }
      writing[buf] = active
      local ok, err = pcall(vim.api.nvim_buf_call, buf, function()
        require("ipynb.native_ui").with_positions(buf, function()
          vim.api.nvim_exec_autocmds("BufWritePre", { buffer = buf, modeline = false })
        end)
        write_notebook(state.jupytext, event.file, nil, buf)
        vim.api.nvim_exec_autocmds("BufWritePost", { pattern = event.file, modeline = false })
      end)
      writing[buf] = nil
      if not ok then error(err, 0) end
      if active.again and vim.api.nvim_buf_is_valid(buf) and vim.bo[buf].modified then
        vim.schedule(function()
          if vim.api.nvim_buf_is_valid(buf) and vim.bo[buf].modified
            and same_file(vim.api.nvim_buf_get_name(buf), active.path) then
            vim.api.nvim_buf_call(buf, function() vim.cmd("update") end)
          end
        end)
      end
    end,
  })
  vim.bo[buf].buftype = ""
end

return M
