local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
vim.opt.rtp:prepend(root)
vim.o.columns, vim.o.lines = 100, 40

local calls = { from_file = {}, clear = {}, render = {} }
package.preload["ipynb.molten_load_image_nvim"] = function()
  return {
    image_api = {
      from_file = function(path, opts)
        calls.from_file[#calls.from_file + 1] = { path = path, opts = opts }
        return opts.id
      end,
      image_size = function() return { width = 8, height = 2 } end,
      render = function(id) calls.render[#calls.render + 1] = id end,
      clear = function(id) calls.clear[#calls.clear + 1] = id end,
      refresh = function() end,
      layout_key = function() return { cell_width = 10, cell_height = 20 } end,
    },
  }
end

local ui = require("ipynb.native_ui")
ui.configure({
  image_provider = "image.nvim",
  output_win_border = { "╭", "─", "╮", "│", "╯", "─", "╰", "│" },
  use_border_highlights = true,
})

local buf = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(buf)
vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "print('image')", "" })
vim.api.nvim_win_set_cursor(0, { 1, 0 })

local function cell(status, success)
  return {
    id = "cell-1",
    begin = { 0, 0 },
    ["end"] = { 1, 0 },
    status = status,
    success = success,
    execution_count = status == "done" and 1 or vim.NIL,
    segments = { { kind = "image", path = "/image.png", mimetype = "image/png" } },
  }
end

local function output_details()
  local ns = vim.api.nvim_get_namespaces()["ipynb-native-output"]
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buf, ns, 0, -1, { details = true })) do
    if mark[4].virt_lines then return mark[4] end
  end
end

local function inline_group()
  local details = assert(output_details(), "expected inline output")
  return details.virt_lines[1][1][2]
end

assert(ui.sync(buf, { cell("hold") }))
assert(inline_group() == "IpynbOutputBorderQueued")
local created, cleared = #calls.from_file, #calls.clear
ui.refresh(buf)
vim.api.nvim_win_set_cursor(0, { 1, 1 })
ui.refresh(buf)
assert(#calls.from_file == created and #calls.clear == cleared,
  "cursor-only refresh must preserve the existing image")

assert(ui.hide(true))
assert(output_details() == nil, "explicit hide must remove inline output")
ui.refresh(buf)
assert(output_details() == nil, "explicit hide must persist across refresh")

assert(ui.show("cell-1"))
local float = vim.api.nvim_get_current_win()
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then float = win break end
end
local border = vim.api.nvim_win_get_config(float).border
assert(border[1][2] == "IpynbOutputBorderQueued")
local float_created, float_cleared = #calls.from_file, #calls.clear
ui.refresh(buf)
assert(#calls.from_file == float_created and #calls.clear == float_cleared,
  "unchanged float refresh must preserve the existing image")
vim.api.nvim_set_current_win(float)
assert(ui.hide(true))
assert(vim.api.nvim_get_current_buf() == buf and output_details() == nil,
  "explicit hide from a focused float must hide its owning cell")

assert(ui.show("cell-1"))
assert(ui.hide())
assert(inline_group() == "IpynbOutputBorderQueued", "temporary float close must restore inline output")

assert(ui.hide(true))
assert(output_details() == nil)
assert(ui.sync(buf, { cell("running") }))
assert(inline_group() == "IpynbOutputBorderRunning", "a new execution must reveal its output")
assert(ui.sync(buf, { cell("done", true) }))
assert(inline_group() == "IpynbOutputBorderSuccess")
assert(ui.sync(buf, { cell("done", false) }))
assert(inline_group() == "IpynbOutputBorderFail")
assert(ui.sync(buf, { cell("new") }))
assert(inline_group() == "IpynbOutputBorder")

ui.clear(buf)
vim.api.nvim_buf_delete(buf, { force = true })
print("pass: native output hide, image stability, and status borders")
vim.cmd("qa!")
