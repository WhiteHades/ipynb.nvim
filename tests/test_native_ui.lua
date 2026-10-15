local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
vim.opt.rtp:prepend(root)
vim.o.columns, vim.o.lines = 100, 40

local calls = { from_file = {}, clear = {}, render = {}, refresh = 0, clear_all = 0 }
local images = {}
package.preload["ipynb.molten_load_image_nvim"] = function()
  return {
    image_api = {
      from_file = function(path, opts)
        calls.from_file[#calls.from_file + 1] = { path = path, opts = opts }
        images[opts.id] = true
        return opts.id
      end,
      image_size = function() return { width = 8, height = 2 } end,
      render = function(id) calls.render[#calls.render + 1] = id end,
      clear = function(id)
        calls.clear[#calls.clear + 1] = id
        images[id] = nil
      end,
      clear_all = function()
        calls.clear_all = calls.clear_all + 1
        images = {}
      end,
      refresh = function()
        calls.refresh = calls.refresh + 1
        for id in pairs(images) do calls.render[#calls.render + 1] = id end
      end,
      layout_key = function() return { cell_width = 10, cell_height = 20 } end,
    },
  }
end

local ui = require("ipynb.native_ui")
ui.shutdown()
assert(package.loaded["ipynb.molten_load_image_nvim"] == nil and package.loaded["ipynb.image_viewer"] == nil,
  "empty shutdown must not load an adapter or viewer")
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

local other = vim.api.nvim_create_buf(false, true)
vim.cmd("vsplit")
vim.api.nvim_set_current_buf(other)
vim.api.nvim_buf_set_lines(other, 0, -1, false, { "print('saved')", "" })
local saved = cell("done", true)
saved.id, saved.old = "saved-cell", true
assert(ui.sync(other, { saved }))
local other_image = calls.from_file[#calls.from_file].opts.id
assert(images[other_image])
assert(ui.clear(buf))
assert(#ui.cells(buf) == 0 and #ui.cells(other) == 1 and images[other_image],
  "ordinary clear must preserve another buffer's cells and image")
assert(calls.clear_all == 0, "ordinary clear must not clear the whole provider")

vim.api.nvim_set_current_buf(buf)
assert(ui.sync(buf, { cell("done", true) }))
assert(ui.show("cell-1"))
local output_win
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then output_win = win end
end
assert(output_win)
local float_image = calls.from_file[#calls.from_file].opts.id
assert(ui.image_popup("cell-1"))
local viewer_win = vim.api.nvim_get_current_win()
local viewer_image = calls.from_file[#calls.from_file].opts.id
for _, limit in ipairs({ "output_win_max_height", "output_win_max_width" }) do
  ui.configure({ [limit] = 0 })
  assert(not vim.api.nvim_win_is_valid(output_win) and not images[float_image],
    "an output with no available content space must close its float and clear its images")
  assert(output_details() and images[other_image] and images[viewer_image]
    and vim.api.nvim_win_is_valid(viewer_win) and vim.api.nvim_get_current_win() == viewer_win,
    "small-output fallback must restore inline output and preserve other buffers and the viewer")
  ui.configure({ [limit] = 999999 })
  assert(ui.show("cell-1"))
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if win ~= viewer_win and vim.api.nvim_win_get_config(win).relative ~= "" then output_win = win end
  end
end
images["cached-image-without-cell-record"] = true
local closed_with_images = false
vim.api.nvim_create_autocmd("WinClosed", { callback = function()
  if next(images) then closed_with_images = true end
end })
local rendered, refreshed = #calls.render, calls.refresh
ui.shutdown()
assert(next(images) == nil and calls.clear_all == 1, "shutdown must clear all cached provider images")
assert(not closed_with_images, "all images must be cleared before destroying any output window")
assert(#calls.render == rendered, "shutdown must not render images during batch cleanup")
assert(calls.refresh == refreshed + 1, "only the closed viewer may refresh the already empty provider")
assert(not vim.api.nvim_win_is_valid(output_win) and not vim.api.nvim_win_is_valid(viewer_win))
assert(#ui.cells(buf) == 0 and #ui.cells(other) == 0, "shutdown must clear saved previews without kernels")
assert(vim.deep_equal(vim.api.nvim_buf_get_lines(buf, 0, -1, false), { "print('image')", "" }))
assert(vim.deep_equal(vim.api.nvim_buf_get_lines(other, 0, -1, false), { "print('saved')", "" }))
local removed = #calls.clear
ui.shutdown()
ui.refresh(buf)
assert(not ui.sync(buf, { cell("done", true) }), "queued cells must not recreate output after shutdown")
assert(calls.clear_all == 1 and #calls.clear == removed and #calls.render == rendered,
  "repeated shutdown must be a no-op")
ui.configure({})
assert(ui.sync(buf, { cell("done", true) }), "reconfiguration must allow a restarted engine to render")
ui.configure({ virt_text_output = true, output_virt_lines = true, virt_text_max_lines = 6 })
local preserved_clicks = 0
vim.keymap.set("n", "<LeftMouse>", function() preserved_clicks = preserved_clicks + 1; return "<Ignore>" end,
  { expr = true })
local traceback = cell("done", false)
traceback.id, traceback.segments = "traceback", nil
traceback.lines = { "NameError", "Traceback (most recent call last):", "Cell In[2], line 15", "  failing line",
  "frame 5", "frame 6", "frame 7", "frame 8", "last traceback line" }
assert(ui.sync(buf, { traceback }))
local trace_mark
for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces()["ipynb-native-output"], 0, -1, { details = true })) do
  local more_row
  for index, chunks in ipairs(mark[4].virt_lines or {}) do
    local text = {}
    for _, chunk in ipairs(chunks) do text[#text + 1] = chunk[1] end
    if table.concat(text):find("More lines", 1, true) then more_row = index end
  end
  if more_row then trace_mark = { mark = mark, more_row = more_row }; break end
end
assert(trace_mark, "long inline traceback must show its expandable More lines row")
local source_win = vim.api.nvim_get_current_win()
local base = vim.fn.screenpos(source_win, trace_mark.mark[2] + 1, 1)
local mouse = vim.fn.getmousepos
vim.fn.getmousepos = function()
  return { winid = source_win, screenrow = base.row + trace_mark.more_row, screencol = 1 }
end
local click
for _, map in ipairs(vim.api.nvim_buf_get_keymap(buf, "n")) do
  if map.lhs == "<LeftMouse>" then click = map.callback; break end
end
assert(click, "expandable output must install a click action")
click()
vim.fn.getmousepos = mouse
assert(vim.wait(500, function()
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_config(win).relative ~= "" then return true end
  end
  return false
end), "clicking More lines must open the full output float")
local full_output
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then full_output = vim.api.nvim_win_get_buf(win); break end
end
assert(full_output, "click must create an output buffer")
assert(table.concat(vim.api.nvim_buf_get_lines(full_output, 0, -1, false), "\n"):find("last traceback line", 1, true),
  "expanded output must contain the traceback tail")
vim.fn.getmousepos = function()
  return { winid = source_win, screenrow = base.row, screencol = 1 }
end
click()
vim.fn.getmousepos = mouse
assert(preserved_clicks == 1, "ordinary clicks must still reach the previous global mouse handler")
assert(ui.hide(), "test output float must close before checking scrolled inline output")
local source_pos = trace_mark.mark[2]
local virtual_height = #trace_mark.mark[4].virt_lines
local top_line, top_screen = source_pos + 2, 1 + virtual_height
local getwininfo, screenpos = vim.fn.getwininfo, vim.fn.screenpos
vim.fn.getwininfo = function(win)
  local info = vim.deepcopy(getwininfo(win)[1])
  info.topline, info.winrow = top_line, 1
  return { info }
end
vim.fn.screenpos = function(win, line, col)
  if win == source_win and line == source_pos + 1 then return { row = 0 } end
  if win == source_win and line == top_line and col == 0 then return { row = top_screen } end
  return screenpos(win, line, col)
end
vim.fn.getmousepos = function()
  return { winid = source_win, screenrow = trace_mark.more_row, screencol = 1 }
end
click()
vim.fn.getwininfo, vim.fn.screenpos, vim.fn.getmousepos = getwininfo, screenpos, mouse
assert(vim.wait(500, function()
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_config(win).relative ~= "" then return true end
  end
  return false
end), "the More lines row must remain clickable when its anchor has scrolled into topfill")
assert(table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_get_current_buf(), 0, -1, false), "\n")
  :find("last traceback line", 1, true), "scrolled expansion must retain the traceback tail")
local rhs = "<Cmd>let g:ipynb_mouse_rhs=1<CR>"
vim.api.nvim_buf_set_keymap(buf, "n", "<LeftMouse>", rhs, { noremap = true, silent = true })
assert(ui.hide(), "second output float must close before checking string mouse mappings")
for _, map in ipairs(vim.api.nvim_buf_get_keymap(buf, "n")) do
  if map.lhs == "<LeftMouse>" then click = map.callback; break end
end
vim.fn.getmousepos = function() return { winid = source_win, screenrow = 0, screencol = 1 } end
local mapped_rhs = click()
vim.fn.getmousepos = mouse
assert(mapped_rhs == vim.api.nvim_replace_termcodes(rhs, true, false, true),
  "ordinary clicks must retain a previous string mapping")
vim.api.nvim_buf_set_keymap(buf, "n", "<LeftMouse>", "'mapped'", { expr = true, noremap = true, silent = true })
assert(ui.show("traceback") and ui.hide(), "test output must reopen and close before checking expression mappings")
for _, map in ipairs(vim.api.nvim_buf_get_keymap(buf, "n")) do
  if map.lhs == "<LeftMouse>" then click = map.callback; break end
end
vim.fn.getmousepos = function() return { winid = source_win, screenrow = 0, screencol = 1 } end
local mapped_expression = click()
vim.fn.getmousepos = mouse
assert(mapped_expression == "mapped", "ordinary clicks must retain a previous expression mapping")
ui.shutdown()
ui.configure({})
package.loaded["ipynb.molten_load_image_nvim"].image_api.clear_all = nil
rendered = #calls.render
ui.shutdown()
assert(next(images) == nil and #calls.render == rendered, "record cleanup must support providers without clear_all")
vim.api.nvim_buf_delete(buf, { force = true })
vim.api.nvim_buf_delete(other, { force = true })
print("pass: native output hide, image stability, status borders, and batch shutdown")
vim.cmd("qa!")
