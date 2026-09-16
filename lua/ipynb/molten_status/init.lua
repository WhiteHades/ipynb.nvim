
-- Ipynb status line functions. Just wrappers around vim functions exposed by the python plugin
local M = {}

---Display a string when the plugin is initialized, and "" otherwise
---@return string
M.initialized = function()
  return vim.fn.IpynbStatusLineInit()
end

---Display the running kernels attached to the current buffer
---@return string
M.kernels = function()
  return vim.fn.IpynbStatusLineKernels(true)
end

---Display all running kernels
---@return string
M.all_kernels = function()
  return vim.fn.IpynbStatusLineKernels()
end

return M
