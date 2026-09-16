local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
local plugins = assert(vim.env.IPYNB_TEST_PLUGINS, "set IPYNB_TEST_PLUGINS to installed lazy plugins")
vim.opt.rtp:prepend(plugins .. "/lazy.nvim")
vim.opt.rtp:append(plugins .. "/nvim-treesitter/runtime")
if vim.env.IPYNB_TEST_SITE then vim.opt.rtp:append(vim.env.IPYNB_TEST_SITE) end
vim.g.maplocalleader = "\\"
local specs = dofile(root .. "/lazy.lua")
local notebook = specs[1]
notebook.dir = root
notebook.build = false
notebook.opts = {
  python = assert(vim.env.IPYNB_TEST_PYTHON),
  images = false,
}
for _, dep in ipairs(notebook.dependencies) do
  dep.dir = plugins .. "/" .. dep[1]:match("/([^/]+)$")
  dep.build = false
end
require("lazy").setup({
  spec = specs,
  root = plugins,
  lockfile = vim.fn.stdpath("state") .. "/test-lazy-lock.json",
  install = { missing = false },
  checker = { enabled = false },
  change_detection = { enabled = false },
  pkg = { enabled = false },
  rocks = { enabled = false },
})
-- lazy resets runtimepath; the fixture reuses already-installed parser binaries.
vim.opt.rtp:append(plugins .. "/nvim-treesitter/runtime")
if vim.env.IPYNB_TEST_SITE then vim.opt.rtp:prepend(vim.env.IPYNB_TEST_SITE) end
