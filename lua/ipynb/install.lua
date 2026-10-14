local M = {}
local pending = false

function M.path()
  for _, tool in ipairs({ { "uv", "~/.local/bin" }, { "cargo", "~/.cargo/bin" } }) do
    local directory = vim.fn.expand(tool[2])
    if vim.fn.executable(tool[1]) == 0 and not vim.tbl_contains(vim.split(vim.env.PATH or "", ":"), directory) then
      vim.env.PATH = (vim.env.PATH and vim.env.PATH ~= "" and vim.env.PATH .. ":" or "") .. directory
    end
  end
end

function M.run(root, options, install)
  if pending then error("Notebook installation is already running") end
  if vim.fn.has("nvim-0.12") == 0 then error("Restart with Neovim 0.12 or newer before installing") end
  local command = { "bash", root .. "/scripts/prerequisites.sh", "--check" }
  if not options.images then table.insert(command, "--no-images") end
  local result = vim.system(command, { text = true }):wait()
  if result.code == 0 then return install() end

  command[3] = "--install"
  if #vim.api.nvim_list_uis() == 0 then
    error((result.stdout or "") .. (result.stderr or "") .. "\nRun in a terminal: "
      .. table.concat(vim.tbl_map(vim.fn.shellescape, command), " ") .. "\nThen retry :IpynbInstall.")
  end

  pending = true
  local async = package.loaded["lazy.async"]
  local task = async and async.running()
  local code
  local function finish()
    pending = false
    if code ~= 0 then error("Prerequisite installation failed or was cancelled. Check the install terminal and retry :IpynbInstall.") end
    vim.env.PATH = vim.fn.expand("~/.local/bin") .. ":" .. vim.fn.expand("~/.cargo/bin")
      .. (vim.env.PATH and vim.env.PATH ~= "" and ":" .. vim.env.PATH or "")
    command[3] = "--check"
    local checked = vim.system(command, { text = true }):wait()
    if checked.code ~= 0 then error((checked.stdout or "") .. (checked.stderr or "")) end
    return install()
  end
  vim.cmd("botright 15new")
  local job = vim.fn.jobstart(command, { term = true, on_exit = function(_, status)
    code = status
    vim.schedule(function()
      if task then task:resume() else
        local ok, err = pcall(finish)
        if not ok then vim.notify(tostring(err), vim.log.levels.ERROR) end
      end
    end)
  end })
  if job <= 0 then pending = false; error("Could not start the prerequisite installer") end
  vim.cmd.startinsert()
  if task then
    task:suspend()
    return finish()
  end
end

return M
