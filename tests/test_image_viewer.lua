local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
vim.opt.rtp:prepend(root)
vim.o.columns, vim.o.lines = 120, 50

local calls = { from_file = {}, clear = {}, render = {}, refresh = 0 }
package.preload["ipynb.molten_load_image_nvim"] = function()
  return {
    image_api = {
      from_file = function(path, opts)
        calls.from_file[#calls.from_file + 1] = { path = path, opts = opts }
        if path == "/bad.png" then return nil end
        return opts.id
      end,
      image_size = function()
        return { width = 1000, height = 1000 }
      end,
      render = function(id)
        calls.render[#calls.render + 1] = id
      end,
      clear = function(id)
        calls.clear[#calls.clear + 1] = id
      end,
      refresh = function()
        calls.refresh = calls.refresh + 1
      end,
    },
  }
end
package.loaded["ipynb.molten_load_image_nvim"] = nil

local viewer = require("ipynb.image_viewer")
local source_window = vim.api.nvim_get_current_win()
assert(viewer.open("/image.png"))

local image_window = vim.api.nvim_get_current_win()
local image_buffer = vim.api.nvim_win_get_buf(image_window)
local image_config = vim.api.nvim_win_get_config(image_window)
assert(vim.bo[image_buffer].filetype == "ipynb_output")
assert(image_config.border[1] == "╭" and image_config.zindex > 50)
assert(image_config.width + 2 <= math.floor(vim.o.columns * 0.84))
assert(image_config.height + 2 <= math.floor(vim.o.lines * 0.84))
assert(image_config.col == math.floor((vim.o.columns - image_config.width - 2) / 2))
assert(image_config.row == math.floor((vim.o.lines - image_config.height - 2) / 2))
assert(vim.api.nvim_buf_line_count(image_buffer) == image_config.height)
assert(calls.from_file[1].opts.viewer and calls.from_file[1].opts.x == 0 and calls.from_file[1].opts.y == 0)
assert(vim.fn.maparg("q", "n", false, true).buffer == 1)
assert(vim.fn.maparg("<Esc>", "n", false, true).buffer == 1)
local localleader = vim.g.maplocalleader or "\\"
assert(vim.fn.maparg(localleader .. "O", "n", false, true).buffer == 1)

vim.o.columns, vim.o.lines = 160, 60
vim.api.nvim_exec_autocmds("VimResized", {})
image_config = vim.api.nvim_win_get_config(image_window)
assert(image_config.width + 2 <= math.floor(vim.o.columns * 0.84))
assert(image_config.height + 2 <= math.floor(vim.o.lines * 0.84))
assert(image_config.height > 20 and vim.api.nvim_buf_line_count(image_buffer) == image_config.height)
assert(#calls.clear >= 1 and #calls.from_file >= 2)

viewer.close()
assert(vim.api.nvim_get_current_win() == source_window)
assert(not vim.api.nvim_win_is_valid(image_window))
assert(calls.refresh >= 2)

assert(not viewer.open("/bad.png"))
assert(#vim.api.nvim_tabpage_list_wins(0) == 1)
print("pass: image viewer bounds, resize, close, and failure cleanup")
vim.cmd("qa!")
