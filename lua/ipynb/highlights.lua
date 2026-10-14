local M = {}

function M.setup()
  local normal = vim.api.nvim_get_hl(0, { name = "Normal", link = false })
  local code = vim.api.nvim_get_hl(0, { name = "RenderMarkdownCode", link = false })
  local background = code.bg
  if not background and normal.bg then
    local red, green, blue = math.floor(normal.bg / 65536), math.floor(normal.bg / 256) % 256, normal.bg % 256
    background = math.floor(red * 0.92) * 65536 + math.floor(green * 0.92) * 256 + math.floor(blue * 0.92)
  end
  local links = {
    IpynbOutputBorder = "FloatBorder",
    IpynbOutputBorderQueued = "DiagnosticWarn",
    IpynbOutputBorderRunning = "DiagnosticInfo",
    IpynbOutputBorderSuccess = "DiagnosticOk",
    IpynbOutputBorderFail = "DiagnosticError",
    IpynbOutputBorderStale = "Comment",
    IpynbOutputWin = "Normal",
    IpynbOutputWinNC = "Normal",
    IpynbOutputFooter = "MoreMsg",
    IpynbVirtualText = "Normal",
    IpyNotebookCodeLabel = "Title",
    IpyNotebookCodeBorder = "FloatBorder",
  }
  for group, link in pairs(links) do
    local attributes = vim.api.nvim_get_hl(0, { name = link, link = false })
    attributes.bg = background
    vim.api.nvim_set_hl(0, group, attributes)
  end
  vim.api.nvim_set_hl(0, "IpyNotebookCodeBlock", { bg = background })
end

return M
