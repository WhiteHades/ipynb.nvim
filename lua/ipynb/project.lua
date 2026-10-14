local M = {}
local pending = {}
local project_markers = {
  "pyproject.toml", "uv.lock", ".venv", "pyrightconfig.json", "setup.py", "setup.cfg", "requirements.txt", "Pipfile",
}

-- Resolve from the notebook, never the editor's changing working directory.
function M.resolve(bufnr, options, requested)
  options = options or {}
  local path = vim.api.nvim_buf_get_name(bufnr)
  local directory = vim.fs.dirname(vim.fn.fnamemodify(path, ":p"))
  local root = vim.fs.root(directory, { vim.list_extend({ ".git" }, project_markers) })
  if root and vim.uv.fs_stat(root .. "/.git") then
    local project = false
    for _, marker in ipairs(project_markers) do
      if vim.uv.fs_stat(root .. "/" .. marker) then project = true; break end
    end
    if not project then root = nil end
  end
  root = root or directory
  local result = { root = root, cwd = root }
  local metadata = vim.b[bufnr].ipynb_read_kernel
  local forced_project = requested == "project"
  local kernel = forced_project and "python3" or requested or options.kernel or (metadata ~= false and metadata or nil)
  local language = vim.b[bufnr].ipynb_language
  if not forced_project and (requested or options.kernel or (kernel and kernel ~= "python3") or (language and not language:match("^python"))) then
    result.kernel, result.reason = kernel, "explicit or saved kernel"
    return result
  end
  local venv = root .. "/.venv"
  for _, executable in ipairs({ venv .. "/bin/python", venv .. "/Scripts/python.exe" }) do
    if vim.fn.executable(executable) == 1 then
      result.python, result.kernel, result.reason = executable, "project", "project .venv"
      return result
    end
  end
  result.kernel, result.reason = "project", "local .venv"
  result.error = "Project Python is missing in " .. venv .. ". Run uv sync from " .. root .. ", then retry."
  return result
end

-- Setup runs only after a run request, with one job per project directory.
function M.prepare(selection, options, callback)
  if selection.kernel ~= "project" then callback(); return end
  if options.auto_setup == false then callback(selection.error); return end
  local root = selection.root
  if vim.fn.isdirectory(root) == 0 and vim.fn.mkdir(root, "p") == 0 then
    callback("Cannot create notebook directory " .. root)
    return
  end
  if pending[root] then table.insert(pending[root], callback); return end
  pending[root] = { callback }
  local function finish(err)
    local callbacks = pending[root]
    pending[root] = nil
    for _, done in ipairs(callbacks) do
      local ok, failure = pcall(done, err)
      if not ok then vim.notify(tostring(failure), vim.log.levels.ERROR) end
    end
  end
  local function run(command, next_step)
    local ok, err = pcall(vim.system, command, {
      text = true, cwd = root, timeout = 600000, env = { UV_PROJECT_ENVIRONMENT = root .. "/.venv" },
    }, function(result)
      vim.schedule(function()
        if result.code ~= 0 then
          finish("Notebook Python setup failed in " .. root .. ": " .. (result.stderr or "Run uv sync to see the error."))
        else
          local success, failure = pcall(next_step)
          if not success then finish(tostring(failure)) end
        end
      end)
    end)
    if not ok then finish(tostring(err)) end
  end
  local function need_uv()
    if vim.fn.executable("uv") == 1 then return true end
    finish("Install uv to set up notebook Python, or create " .. root .. "/.venv with ipykernel yourself.")
    return false
  end
  local function install_kernel()
    if not need_uv() then return end
    vim.notify("Setting up notebook Python in " .. root .. "/.venv", vim.log.levels.INFO)
    local command = vim.uv.fs_stat(root .. "/pyproject.toml")
      and { "uv", "add", "--project", root, "--dev", "ipykernel" }
      or { "uv", "pip", "install", "--python", selection.python, "ipykernel" }
    if command[2] == "add" then vim.notify("Adding ipykernel to " .. root .. "/pyproject.toml", vim.log.levels.INFO) end
    run(command, function() finish() end)
  end
  local function check_kernel()
    local resolved = M.resolve(selection.bufnr or 0, options, "project")
    selection.python = resolved.python
    if not selection.python then finish(resolved.error); return end
    local ok, err = pcall(vim.system, { selection.python, "-c", "import ipykernel_launcher" }, { text = true, timeout = 10000 }, function(result)
      vim.schedule(function()
        if result.code == 0 then finish() else install_kernel() end
      end)
    end)
    if not ok then finish(tostring(err)) end
  end
  if selection.python then check_kernel(); return end
  if not need_uv() then return end
  if vim.uv.fs_stat(root .. "/.venv") then finish(selection.error); return end
  vim.notify("Setting up notebook Python in " .. root .. "/.venv", vim.log.levels.INFO)
  if vim.uv.fs_stat(root .. "/pyproject.toml") then
    local command = { "uv", "sync", "--project", root }
    if vim.uv.fs_stat(root .. "/uv.lock") then table.insert(command, "--locked") end
    run(command, check_kernel)
  elseif vim.uv.fs_stat(root .. "/uv.lock") then
    finish("Cannot set up " .. root .. ": uv.lock exists without pyproject.toml.")
  elseif vim.uv.fs_stat(root .. "/requirements.txt") or vim.uv.fs_stat(root .. "/setup.py") or vim.uv.fs_stat(root .. "/setup.cfg") then
    run({ "uv", "venv", "--python", options.python, root .. "/.venv" }, function()
      local resolved = M.resolve(selection.bufnr or 0, options, "project")
      selection.python = resolved.python
      local command = { "uv", "pip", "install", "--python", selection.python, "ipykernel" }
      if vim.uv.fs_stat(root .. "/requirements.txt") then
        vim.list_extend(command, { "-r", root .. "/requirements.txt" })
      end
      run(command, function() finish() end)
    end)
  else
    run({ "uv", "init", "--bare", "--vcs", "none", "--name", "notebook-env", "--python", options.python, root }, install_kernel)
  end
end

function M.python(bufnr, options)
  if options.kernel_python then return options.kernel_python end
  local path = vim.api.nvim_buf_get_name(bufnr)
  if path:match("%.ipynb%.otter%.py$") then
    local parent = vim.fn.bufnr(path:gsub("%.otter%.py$", ""))
    if parent >= 0 then bufnr = parent end
  end
  if vim.b[bufnr].ipynb_execution_python then return vim.b[bufnr].ipynb_execution_python end
  return M.resolve(bufnr, options).python
end

return M
