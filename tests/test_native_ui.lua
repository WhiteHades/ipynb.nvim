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
ui.configure({ image_provider = "none" })
local full_output = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(full_output)
vim.api.nvim_buf_set_lines(full_output, 0, -1, false, { "print('rows')", "" })
local rows = {}
for index = 1, 10000 do rows[index] = "output row " .. index end
local function rendered_rows()
  assert(ui.sync(full_output, { { id = "full-output", begin = { 0, 0 }, ["end"] = { 0, 13 },
    status = "done", success = true, lines = rows } }))
  local text = {}
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(full_output,
    vim.api.nvim_get_namespaces()["ipynb-native-output"], 0, -1, { details = true })) do
    for _, line in ipairs(mark[4].virt_lines or {}) do
      for _, chunk in ipairs(line) do text[#text + 1] = chunk[1] end
    end
  end
  return table.concat(text, "\n")
end
local complete = rendered_rows()
assert(complete:find("output row 10000", 1, true) and not complete:find("More lines", 1, true),
  "ordinary output must remain complete through the default 10,000 row boundary")
rows[#rows + 1] = "output row 10001"
local bounded = rendered_rows()
assert(bounded:find("output row 1", 1, true) and bounded:find("More lines", 1, true)
  and not bounded:find("output row 10001", 1, true), "runaway output must retain a bounded preview")
assert(vim.deep_equal(vim.api.nvim_buf_get_lines(full_output, 0, -1, false), { "print('rows')", "" }),
  "output limits must not change notebook source")
ui.clear(full_output)
vim.api.nvim_buf_delete(full_output, { force = true })
local formatted = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_lines(formatted, 0, -1, false, { "```python", "print(1)", "```", "", "```python", "print(2)", "```" })
ui.sync(formatted, {
  { id = "format-first", begin = { 1, 0 }, ["end"] = { 1, 8 } },
  { id = "format-second", begin = { 5, 0 }, ["end"] = { 5, 8 } },
})
vim.api.nvim_set_current_buf(formatted)
vim.api.nvim_win_set_cursor(0, { 6, 3 })
ui.with_positions(formatted, function()
  vim.api.nvim_buf_set_lines(formatted, 0, -1, false,
    { "```python", "print(1)", "# formatted", "```", "", "```python", "print(2)", "```" })
end)
assert(vim.deep_equal(vim.api.nvim_win_get_cursor(0), { 7, 3 }),
  "save hooks inserting lines above the cursor must keep it on the same code")
local preserved = ui.positions(formatted)
assert(preserved[1].begin[1] == 1 and preserved[2].begin[1] == 6,
  "whole-buffer save hooks must preserve cell identities across inserted lines")
assert(preserved[1].source == "print(1)\n# formatted" and preserved[2].source == "print(2)")
assert(not pcall(ui.with_positions, formatted, function() error("save hook failed") end),
  "save hook errors must still abort saving")
vim.api.nvim_buf_set_lines(formatted, 1, 3, false, {})
local empty = ui.positions(formatted)[1]
assert(empty.source == "" and vim.deep_equal(empty.begin, empty["end"]),
  "deleting every body line must not include the closing fence in cell source")
assert(ui.current(formatted, empty.begin[1], 0).id == "format-first",
  "the closing fence of a bodyless cell must still select its output")
ui.clear(formatted)
ui.sync(formatted, {{id = "empty", begin = {1, 0}, ["end"] = {1, 0}}})
vim.api.nvim_buf_set_lines(formatted, 1, 1, false, {"print(3)"})
assert(ui.positions(formatted)[1].source == "print(3)",
  "typing into an imported bodyless cell must exclude its closing fence")
for _, fence in ipairs({"```", "````", "`````"}) do
  ui.clear(formatted)
  vim.api.nvim_buf_set_lines(formatted, 0, -1, false, {fence .. "python", "print(3)", fence})
  ui.sync(formatted, {{id = "last", begin = {1, 0}, ["end"] = {1, 8}}})
  vim.api.nvim_buf_set_lines(formatted, 1, 2, false, {})
  local last = ui.positions(formatted)[1]
  assert(last.source == "" and vim.deep_equal(last.begin, last["end"])
    and ui.current(formatted, 1, 0).id == "last",
    "clearing the final cell at EOF must leave an empty selectable cell")
  vim.api.nvim_buf_set_lines(formatted, 1, 1, false, {"print(4)"})
  assert(ui.positions(formatted)[1].source == "print(4)" and ui.current(formatted, 2, 0).id == "last",
    "typing into a cleared cell must exclude its closing fence for " .. fence)
end
ui.clear(formatted)
vim.api.nvim_buf_delete(formatted, { force = true })
ui.shutdown()
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
  local gap = details.virt_lines[1]
  local gap_text = table.concat(vim.tbl_map(function(chunk) return chunk[1] end, gap))
  assert(gap[1][2] == "Normal" and gap_text ~= "" and gap_text:match("^ *$"),
    "inline output must have a full-width normal-background gap above it")
  return details.virt_lines[2][1][2]
end

assert(ui.sync(buf, { cell("hold") }))
assert(inline_group() == "IpynbOutputBorderQueued")
local image_options = calls.from_file[#calls.from_file].opts
local image_anchor = vim.api.nvim_buf_get_extmarks(buf,
  vim.api.nvim_get_namespaces()["ipynb-native-output"], 0, -1, {})[1][2]
assert(image_options.y == image_anchor and image_options.render_offset_top == 2,
  "the gap must move images within virtual output, not onto the next buffer line")
assert(image_options.virtual_layout.virtual_height == #output_details().virt_lines,
  "image clipping must count the normal-background gap when output is partly scrolled")
-- Converter spans can become available after an Insert refresh at the same tick.
local cells = package.loaded['ipynb.cells']
vim.api.nvim_buf_set_lines(buf, -1, -1, false, {'', ''})
package.loaded['ipynb.cells'] = {spans=function()
  return {{begin={0,0}, ['end']={2,0}}}
end}
vim.b[buf].ipynb_code_spans_tick = nil
ui.refresh(buf)
local function anchor()
  return vim.api.nvim_buf_get_extmarks(buf,
    vim.api.nvim_get_namespaces()['ipynb-native-output'], 0, -1, {})[1][2]
end
assert(anchor() == 3)
vim.b[buf].ipynb_code_spans = {{begin={0,0}, ['end']={1,0}}}
vim.b[buf].ipynb_code_spans_tick = vim.api.nvim_buf_get_changedtick(buf)
ui.refresh(buf)
assert(anchor() == 2, 'fresh converter spans must replace same-tick fallback spans')
vim.b[buf].ipynb_code_spans = {{begin={0,0}, ['end']={2,0}}}
ui.refresh(buf)
assert(anchor() == 3, 'forced converter refresh must replace same-tick executable spans')
vim.b[buf].ipynb_code_spans, vim.b[buf].ipynb_code_spans_tick = nil, nil
package.loaded['ipynb.cells'] = cells
vim.api.nvim_buf_set_lines(buf, 2, -1, false, {})
ui.refresh(buf)
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

vim.wo.number = true
assert(ui.show("cell-1"))
local float = vim.api.nvim_get_current_win()
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then float = win break end
end
local border = vim.api.nvim_win_get_config(float).border
assert(border[1][2] == "IpynbOutputBorderQueued")
local config = vim.api.nvim_win_get_config(float)
local source_info = vim.fn.getwininfo(config.win)[1]
assert(config.col >= source_info.textoff, "output float must leave the line number gutter visible")
local code_bottom = vim.fn.screenpos(config.win, 2, 1).row - source_info.winrow
assert(config.row >= code_bottom + 2, "output float must begin below its code with a normal background gap")
local screenpos = vim.fn.screenpos
vim.fn.screenpos = function() return { row = 0, col = 0 } end
ui.refresh(buf)
assert(vim.api.nvim_win_is_valid(float) and vim.api.nvim_win_get_config(float).hide,
  "an offscreen output inspector must hide without losing its window")
vim.fn.screenpos = screenpos
ui.refresh(buf)
assert(not vim.api.nvim_win_get_config(float).hide, "the inspector must return with its source anchor")
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
local second_traceback = vim.deepcopy(traceback)
second_traceback.id, second_traceback.begin, second_traceback["end"] = "traceback-2", { 2, 0 }, { 3, 0 }
vim.api.nvim_buf_set_lines(buf, -1, -1, false, { "print('second trace')", "" })
local unrelated_mouse_buf = vim.api.nvim_create_buf(false, true)
vim.keymap.set("n", "<LeftMouse>", function() error("another buffer's mouse handler was used") end,
  { buffer = unrelated_mouse_buf, expr = true })
vim.keymap.set("n", "zz", "<Plug>(ipynb-toggle-output)", { buffer = buf, desc = "test output toggle" })
local get_buffer_maps, get_global_maps = vim.api.nvim_buf_get_keymap, vim.api.nvim_get_keymap
local mapping_scans = 0
vim.api.nvim_buf_get_keymap = function(...)
  mapping_scans = mapping_scans + 1
  return get_buffer_maps(...)
end
vim.api.nvim_get_keymap = function(...)
  mapping_scans = mapping_scans + 1
  return get_global_maps(...)
end
vim.api.nvim_buf_call(unrelated_mouse_buf, function()
  assert(ui.sync(buf, { traceback, second_traceback }))
  assert(vim.api.nvim_get_current_buf() == unrelated_mouse_buf, "output refresh must preserve the current buffer")
end)
vim.api.nvim_buf_get_keymap, vim.api.nvim_get_keymap = get_buffer_maps, get_global_maps
assert(vim.api.nvim_get_current_buf() == buf, "background output lookup must restore the original buffer")
assert(mapping_scans == 2, "truncated output must resolve the actual toggle key once per refresh")
vim.api.nvim_buf_delete(unrelated_mouse_buf, { force = true })
vim.keymap.del("n", "zz", { buffer = buf })
vim.keymap.set("n", "xx", "<Plug>(ipynb-toggle-output)")
vim.keymap.set("n", "xx", "<Nop>", { buffer = buf })
vim.keymap.set("n", "yy", "<Plug>(ipynb-toggle-output)", { buffer = buf, desc = "test output toggle" })
ui.refresh(buf)
vim.keymap.del("n", "xx")
local trace_mark
for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces()["ipynb-native-output"], 0, -1, { details = true })) do
  local more_row
  for index, chunks in ipairs(mark[4].virt_lines or {}) do
    local text = {}
    for _, chunk in ipairs(chunks) do text[#text + 1] = chunk[1] end
    local line = table.concat(text)
    if line:find("More lines", 1, true) then
      assert(line:find("yy", 1, true) and not line:find("zz", 1, true) and not line:find("xx", 1, true),
        "More lines must refresh to the currently mapped output toggle key")
      more_row = index
    end
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
local full_output, trace_float
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then
    full_output, trace_float = vim.api.nvim_win_get_buf(win), win
    break
  end
end
assert(full_output, "click must create an output buffer")
assert(table.concat(vim.api.nvim_buf_get_lines(full_output, 0, -1, false), "\n"):find("last traceback line", 1, true),
  "expanded output must contain the traceback tail")
assert(vim.api.nvim_get_current_win() == trace_float, "opening output must focus the output view")
vim.api.nvim_win_set_cursor(trace_float, { 5, 2 })
vim.api.nvim_win_call(trace_float, function()
  local view = vim.fn.winsaveview()
  view.topline, view.lnum, view.col = 3, 5, 2
  vim.fn.winrestview(view)
end)
local output_view = vim.api.nvim_win_call(trace_float, vim.fn.winsaveview)
local output_maps = vim.api.nvim_buf_get_keymap(full_output, "n")
local return_to_source, toggle_output
for _, map in ipairs(output_maps) do
  if map.lhs == "q" or map.lhs == "<Esc>" then return_to_source = map.callback end
  if map.lhs == "yy" then toggle_output = map.callback end
end
assert(return_to_source and toggle_output, "output view must map return and the notebook's actual toggle key")
return_to_source()
assert(vim.api.nvim_get_current_win() == source_win and not vim.api.nvim_win_is_valid(trace_float),
  "q or Esc must return to the source and close the detached output window")
local function anchored_output_text()
  local ns = vim.api.nvim_get_namespaces()["ipynb-native-output"]
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buf, ns,
      { trace_mark.mark[2], 0 }, { trace_mark.mark[2], -1 }, { details = true })) do
    if mark[4].virt_lines then
      local text = {}
      for _, line in ipairs(mark[4].virt_lines) do
        for _, chunk in ipairs(line) do text[#text + 1] = chunk[1] end
        text[#text + 1] = "\n"
      end
      return table.concat(text)
    end
  end
end
local expanded_text = assert(anchored_output_text(), "expanded output must stay attached to its source cell")
assert(expanded_text:find("NameError", 1, true) and expanded_text:find("last traceback line", 1, true)
    and not expanded_text:find("More lines", 1, true),
  "returning from the output browser must show the complete output inline")
assert(ui.open("traceback"), "the source cell must reopen its output browser")
assert(vim.wait(500, function()
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_config(win).relative ~= "" then return true end
  end
  return false
end), "opening an already expanded output must recreate its browser")
local reopened_output, reopened_float
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then
    reopened_output, reopened_float = vim.api.nvim_win_get_buf(win), win
  end
end
assert(reopened_output and table.concat(vim.api.nvim_buf_get_lines(reopened_output, 0, -1, false), "\n")
    :find("last traceback line", 1, true), "reopening the browser must retain the full output")
local reopened_view = vim.api.nvim_win_call(reopened_float, vim.fn.winsaveview)
assert(reopened_view.topline == output_view.topline and reopened_view.lnum == output_view.lnum
    and reopened_view.col == output_view.col, "reopening must restore the output cursor and scroll position")
output_maps = vim.api.nvim_buf_get_keymap(reopened_output, "n")
return_to_source, toggle_output = nil, nil
for _, map in ipairs(output_maps) do
  if map.lhs == "q" or map.lhs == "<Esc>" then return_to_source = map.callback end
  if map.lhs == "yy" then toggle_output = map.callback end
end
assert(return_to_source and toggle_output, "reopened output must keep return and mapped toggle keys")
return_to_source()
assert(vim.api.nvim_get_current_win() == source_win and not vim.api.nvim_win_is_valid(reopened_float),
  "the output browser must always return to its source without a detached window")
assert(anchored_output_text() == expanded_text, "reopening and leaving the browser must preserve expanded output")
local filler = {}
for index = 1, 80 do filler[index] = "scroll filler " .. index end
vim.api.nvim_buf_set_lines(buf, -1, -1, false, filler)
vim.api.nvim_win_set_cursor(source_win, { vim.api.nvim_buf_line_count(buf), 0 })
vim.api.nvim_win_call(source_win, function() vim.cmd("normal! zt") end)
assert(vim.fn.screenpos(source_win, trace_mark.mark[2] + 1, 1).row == 0,
  "the output source anchor must be above the viewport during the scroll regression check")
ui.refresh(buf)
vim.api.nvim_win_set_cursor(source_win, { trace_mark.mark[2] + 1, 0 })
vim.api.nvim_win_call(source_win, function() vim.cmd("normal! zt") end)
ui.refresh(buf)
assert(anchored_output_text() == expanded_text,
  "scrolling the expanded output anchor out of view and back must preserve every line")
assert(ui.open("traceback"), "the output browser must reopen after scrolling")
reopened_float = nil
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then reopened_float = win end
end
assert(reopened_float and vim.api.nvim_get_current_win() == reopened_float,
  "opening output must focus the browser")
reopened_output = vim.api.nvim_win_get_buf(reopened_float)
for _, map in ipairs(vim.api.nvim_buf_get_keymap(reopened_output, "n")) do
  if map.lhs == "yy" then toggle_output = map.callback end
end
assert(toggle_output, "reopened output must keep the remapped toggle")
toggle_output()
assert(vim.api.nvim_get_current_win() == source_win and not vim.api.nvim_win_is_valid(reopened_float),
  "the mapped toggle must collapse expanded output and return to the source")
local collapsed_text = assert(anchored_output_text(), "collapsed output must restore its inline preview")
assert(collapsed_text:find("More lines", 1, true) and not collapsed_text:find("last traceback line", 1, true),
  "collapsing expanded output must restore the truncated preview")
vim.fn.getmousepos = function()
  return { winid = source_win, screenrow = base.row, screencol = 1 }
end
click()
vim.fn.getmousepos = mouse
assert(preserved_clicks == 1, "ordinary clicks must still reach the previous global mouse handler")
assert(not ui.hide(), "keyboard toggle already closes output before checking scrolled inline output")
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
toggle_output()
local output_still_open = false
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then output_still_open = true end
end
assert(not output_still_open, "the mapped toggle must collapse before custom mouse mappings are checked")
local rhs = "<Cmd>let g:ipynb_mouse_rhs=1<CR>"
vim.api.nvim_buf_set_keymap(buf, "n", "<LeftMouse>", rhs, { noremap = true, silent = true })
assert(ui.show("traceback") and ui.hide(), "output must redraw after installing a string mouse fallback")
for _, map in ipairs(vim.api.nvim_buf_get_keymap(buf, "n")) do
  if map.lhs == "<LeftMouse>" then click = map.callback; break end
end
vim.fn.getmousepos = function() return { winid = source_win, screenrow = 0, screencol = 1 } end
assert(click, "string mouse fallback must remain wrapped")
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
ui.configure({ enter_output_behavior = "open_then_enter", virt_text_output = true })
vim.api.nvim_win_set_cursor(source_win, { vim.api.nvim_buf_line_count(buf), 0 })
vim.api.nvim_win_call(source_win, function() vim.cmd("normal! zt") end)
assert(ui.open("traceback") and vim.api.nvim_get_current_win() == source_win and anchored_output_text()
    :find("last traceback line", 1, true),
  "open_then_enter must first show the complete output inline without moving focus")
assert(vim.api.nvim_win_get_cursor(source_win)[1] < vim.api.nvim_buf_line_count(buf) - 20,
  "opening offscreen output without entering must still reveal its source anchor")
assert(ui.open("traceback"), "the second open_then_enter action must open the browser")
local behavior_float
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then behavior_float = win end
end
assert(behavior_float and vim.api.nvim_get_current_win() == behavior_float,
  "the second open_then_enter action must focus the browser")
for _, map in ipairs(vim.api.nvim_buf_get_keymap(vim.api.nvim_win_get_buf(behavior_float), "n")) do
  if map.lhs == "q" then return_to_source = map.callback end
  if map.lhs == "yy" then toggle_output = map.callback end
end
assert(return_to_source and toggle_output, "configured output modes must retain their keyboard controls")
return_to_source()
assert(anchored_output_text():find("last traceback line", 1, true),
  "q must retain complete inline output after open_then_enter")
assert(ui.toggle_open("traceback") and anchored_output_text():find("More lines", 1, true),
  "the source toggle must collapse expanded output")
ui.configure({ enter_output_behavior = "no_open" })
assert(not ui.open("traceback"), "no_open must continue to prevent opening an output browser")
ui.configure({ enter_output_behavior = "open_and_enter", virt_text_output = false })
assert(ui.open("traceback"), "disabling virtual output must keep the floating browser available")
local configured_float
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then configured_float = win end
end
assert(configured_float, "virt_text_output=false must create the configured output window")
local configured_output = vim.api.nvim_win_get_buf(configured_float)
for _, map in ipairs(vim.api.nvim_buf_get_keymap(configured_output, "n")) do
  if map.lhs == "q" then return_to_source = map.callback end
  if map.lhs == "yy" then toggle_output = map.callback end
end
assert(return_to_source and toggle_output, "floating-only output must keep its keyboard controls")
return_to_source()
assert(vim.api.nvim_get_current_win() == source_win and vim.api.nvim_win_is_valid(configured_float),
  "q must retain the floating output when virtual output is disabled")
toggle_output()
assert(not vim.api.nvim_win_is_valid(configured_float), "the mapped toggle must still close floating-only output")
ui.configure({ virt_text_output = true, enter_output_behavior = "open_and_enter" })
ui.shutdown()
ui.configure({})
package.loaded["ipynb.molten_load_image_nvim"].image_api.clear_all = nil
rendered = #calls.render
ui.shutdown()
assert(next(images) == nil and #calls.render == rendered, "record cleanup must support providers without clear_all")

local previous_snacks, previous_global_snacks = package.loaded.snacks, Snacks
local previous_image_style = package.loaded["ipynb.image_style"]
local snack_placements = {}
local snack_roundings = {}
local snack_style_releases = 0
local delayed_snack_callbacks = {}
local snack_upstream_images = {}
Snacks = {
  image = {
    placement = { new = function(buffer, path, opts)
      local image = snack_upstream_images[path]
      if not image then
        image = { placements = {}, sends = 0, completions = 0, errors = 0 }
        function image:on_ready() self.sends = self.sends + 1 end
        local convert = { finished = not path:match("^/pending"), opts = {} }
        function convert:done() return self.finished end
        convert.opts.on_done = function()
          image.completions = image.completions + 1
          if convert.failed then image.errors = image.errors + 1
          else vim.schedule(function() image:on_ready() end) end
        end
        image._convert = convert
        snack_upstream_images[path] = image
      end
      local placement = { buffer = buffer, path = path, opts = opts, closed = 0, img = image }
      image.placements[placement] = placement
      function placement:close()
        self.closed = self.closed + 1
        self.img.placements[self] = nil
      end
      snack_placements[#snack_placements + 1] = placement
      return placement
    end },
    util = {
      fit = function(_, limits) return { width = limits.width, height = limits.height } end,
      dim = function() return { width = 600, height = 400 } end,
    },
    terminal = { size = function() return { cell_width = 10, cell_height = 20 } end },
  },
}
package.loaded["ipynb.image_style"] = {
  metrics = function(size) return size.cell_width / 2, 1, 1 end,
  rounded = function(path, width, height, radius, source_width, source_height, callback)
    snack_roundings[#snack_roundings + 1] = { path, width, height, radius, source_width, source_height }
    if path:match("^/delayed") then
      delayed_snack_callbacks[#delayed_snack_callbacks + 1] = function()
        callback(path .. ".rounded", function() snack_style_releases = snack_style_releases + 1 end)
      end
    elseif path == "/fallback.png" then callback(nil) else
      callback(path .. ".rounded", function() snack_style_releases = snack_style_releases + 1 end)
    end
  end,
}
package.loaded.snacks = nil
package.preload.snacks = function()
  return {
    config = { image = { doc = { max_width = 80, max_height = 40 } } },
    image = {
      util = {
        fit = function(_, limits) return { width = limits.width, height = limits.height } end,
        dim = function() return { width = 600, height = 400 } end,
      },
      terminal = { size = function() return { cell_width = 10, cell_height = 20 } end },
    },
  }
end
package.loaded["ipynb.molten_load_snacks_nvim"] = nil
local snacks_adapter = require("ipynb.molten_load_snacks_nvim").snacks_api
local first_id = snacks_adapter.from_file("/same.png", {
  id = "same-first", buffer = buf, x = 0, y = 1, max_width = 30, max_height = 12,
})
local second_id = snacks_adapter.from_file("/same.png", {
  id = "same-second", buffer = other, x = 0, y = 1, max_width = 30, max_height = 12,
})
snacks_adapter.render(first_id)
snacks_adapter.render(second_id)
assert(first_id == "same-first" and second_id == "same-second" and #snack_placements == 2,
  "the same image path must support independent placement identifiers")
assert(snack_roundings[1][2] == 300 and snack_roundings[1][3] == 240
    and snack_roundings[1][4] == 1 and snack_roundings[1][5] == 600 and snack_roundings[1][6] == 400
    and snack_placements[1].path == "/same.png.rounded",
  "Snacks float plots must use the shared radius style at the fitted pixel canvas")
assert(vim.deep_equal(snack_placements[1].opts.pos, { 2, 0 })
    and vim.deep_equal(snack_placements[1].opts.range, { 2, 0, 13, 30 })
    and snack_placements[1].opts.width == 30 and snack_placements[1].opts.height == 12
    and snack_placements[1].opts.conceal,
  "Snacks must overlay the exact reserved blank rows at the fitted image size")
snacks_adapter.clear(first_id)
assert(snack_placements[1].closed == 1 and snack_placements[2].closed == 0,
  "clearing one image ID must preserve the other placement for the same path")
assert(snack_style_releases == 1, "clearing a Snacks placement must release its style lease")
snacks_adapter.render(first_id)
assert(#snack_placements == 2 and snacks_adapter.image_size(first_id).width == 0,
  "rendering a cleared image ID must be a no-op")
snacks_adapter.clear_all()
assert(snack_placements[2].closed == 1, "clear_all must remove all remaining placements")
assert(snack_style_releases == 2, "clear_all must release every accepted style lease")
local delayed_id = snacks_adapter.from_file("/delayed-old.png", { id = "delayed", buffer = buf })
snacks_adapter.from_file("/delayed-current.png", { id = delayed_id, buffer = buf })
delayed_snack_callbacks[1]()
assert(snack_style_releases == 3 and #snack_placements == 2,
  "a replaced stale callback must release its lease without creating a placement")
snacks_adapter.render(delayed_id)
delayed_snack_callbacks[2]()
assert(#snack_placements == 3 and snack_placements[3].path == "/delayed-current.png.rounded",
  "the replacement callback must create the current image placement")
snacks_adapter.clear(delayed_id)
assert(snack_placements[3].closed == 1 and snack_style_releases == 4,
  "clearing the replacement must close it and release its accepted lease")
local pending_a = snacks_adapter.from_file("/pending-shared.png", { id = "pending-a", buffer = buf })
local pending_b = snacks_adapter.from_file("/pending-shared.png", { id = "pending-b", buffer = other })
snacks_adapter.render(pending_a)
snacks_adapter.render(pending_b)
local pending_image = snack_upstream_images["/pending-shared.png.rounded"]
snacks_adapter.clear(pending_a)
snacks_adapter.clear(pending_b)
assert(snack_style_releases == 4, "pending conversions must retain closed owners' leases until completion")
pending_image._convert.finished = true
pending_image._convert.failed = true
pending_image._convert.opts.on_done(pending_image._convert)
assert(vim.wait(1000, function() return snack_style_releases == 6 end),
  "failed shared conversion completion must release every closed owner's lease")
assert(pending_image.completions == 1 and pending_image.errors == 1 and pending_image.sends == 0,
  "failed conversion cleanup must preserve its callback without sending a file with no placements")
local live_a = snacks_adapter.from_file("/pending-live.png", { id = "live-a", buffer = buf })
local live_b = snacks_adapter.from_file("/pending-live.png", { id = "live-b", buffer = other })
snacks_adapter.render(live_a)
snacks_adapter.render(live_b)
local live_image = snack_upstream_images["/pending-live.png.rounded"]
snacks_adapter.clear(live_a)
live_image._convert.finished = true
live_image._convert.opts.on_done(live_image._convert)
assert(vim.wait(1000, function() return snack_style_releases == 7 end),
  "conversion completion must release the closed owner while another placement stays live")
assert(live_image.completions == 1 and live_image.sends == 1,
  "a shared live placement must still receive its converted image")
snacks_adapter.clear(live_b)
assert(snack_style_releases == 8, "a closed owner must release immediately after conversion completes")
snack_placements = {}
local fallback_id = snacks_adapter.from_file("/fallback.png", { id = "fallback", buffer = buf })
snacks_adapter.render(fallback_id)
assert(#snack_placements == 1 and snack_placements[1].path == "/fallback.png",
  "failed styling must render the source image")
snacks_adapter.clear_all()
snack_placements = {}
local rich = require("ipynb.native_ui.rich")({
  options = { image_provider = "snacks.nvim" },
  number = function(value, fallback) return tonumber(value) or fallback end,
  notify = function() end,
  valid_win = vim.api.nvim_win_is_valid,
  buffers = {},
})
local provider = rich.provider()
local image_nvim_count = #calls.from_file
local inline = provider.from_file("/inline.png", {
  id = "inline", buffer = buf, x = 1, y = 5, max_width = 30, max_height = 12,
  render_offset_top = 2, virtual_layout = { virtual_height = 8 },
})
assert(#calls.from_file == image_nvim_count + 1 and calls.from_file[#calls.from_file].opts.virtual_layout,
  "Snacks selection must route virtual output through image.nvim's virtual geometry")
assert(#snack_placements == 0, "virtual output must not create a physical row Snacks placement")
local float_buf = vim.api.nvim_create_buf(false, true)
local floating = provider.from_file("/float.png", {
  id = "floating", buffer = float_buf, x = 0, y = 1, max_width = 30, max_height = 12,
})
provider.render(floating)
assert(#snack_placements == 1, "floating output must use the selected Snacks placement")
local snack_opts = snack_placements[1].opts
assert(snack_placements[1].buffer == float_buf and snack_opts.max_width == 30 and snack_opts.max_height == 12
    and snack_opts.width == 30 and snack_opts.height == 12 and snack_opts.conceal
    and snack_opts.auto_resize and vim.deep_equal(snack_opts.pos, { 2, 0 })
    and vim.deep_equal(snack_opts.range, { 2, 0, 13, 30 }),
  "floating Snacks images must use the requested bounds and resize with their display buffer")
assert(provider.image_size(floating).width == 30 and provider.image_size(floating).height == 12,
  "floating Snacks sizing must honor the output bounds")
provider.render(inline)
provider.clear(floating)
assert(snack_placements[1].closed == 1, "clearing floating output must close its Snacks placement")
local floating_again = provider.from_file("/float-again.png", {
  id = "floating-again", buffer = float_buf, x = 0, y = 1, max_width = 24, max_height = 8,
})
provider.render(floating_again)
local refresh_count = calls.refresh
provider.refresh()
assert(calls.refresh == refresh_count + 1
    and vim.deep_equal(provider.layout_key(), { cell_width = 10, cell_height = 20 }),
  "Snacks selection must retain image.nvim refresh and terminal geometry hooks")
local cleared_before_shutdown = #calls.clear
provider.clear_all()
assert(floating_again and snack_placements[2].closed == 1, "Snacks selection cleanup must clear floating placements")
local inline_image_id = calls.from_file[#calls.from_file].opts.id
assert(#calls.clear > cleared_before_shutdown and not images[inline_image_id],
  "Snacks selection cleanup must clear inline image.nvim state")
vim.api.nvim_buf_delete(float_buf, { force = true })
package.loaded.snacks = previous_snacks
package.preload.snacks = nil
package.loaded["ipynb.molten_load_snacks_nvim"] = nil
Snacks = previous_global_snacks
package.loaded["ipynb.image_style"] = previous_image_style
local bottom = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(bottom)
local bottom_lines = {}
for index = 1, 100 do bottom_lines[index] = index == 99 and "print(1)" or "" end
vim.api.nvim_buf_set_lines(bottom, 0, -1, false, bottom_lines)
vim.api.nvim_win_set_cursor(0, { 99, 0 })
ui.configure({ image_provider = "none", virt_text_output = true, output_virt_lines = true })
ui.sync(bottom, { { id = "bottom", begin = { 98, 0 }, ["end"] = { 98, 8 },
  status = "done", lines = traceback.lines } })
vim.cmd("normal! zb")
vim.cmd("redraw")
assert(ui.open("bottom"), "output near the bottom must scroll its source anchor into space before opening")
local inspector
for _, win in ipairs(vim.api.nvim_list_wins()) do
  if vim.api.nvim_win_get_config(win).relative ~= "" then inspector = win end
end
assert(inspector and vim.api.nvim_win_get_config(inspector).row > 0)
ui.clear(bottom)
vim.api.nvim_buf_delete(bottom, { force = true })
local large = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(large)
local source, spans, many = {}, {}, {}
for index = 1, 100 do
  local row = #source + 1
  vim.list_extend(source, { "```python", "x=1", "```", "" })
  spans[index] = { begin = { row, 0 }, ["end"] = { row, 3 } }
  many[index] = { id = index + 500, begin = { row, 0 }, ["end"] = { row, 3 },
    source = "x=1", status = "new", lines = {}, segments = {} }
end
vim.api.nvim_buf_set_lines(large, 0, -1, false, source)
vim.b[large].ipynb_code_spans = spans
vim.b[large].ipynb_code_spans_tick = vim.api.nvim_buf_get_changedtick(large)
ui.configure({ image_provider = "none", output_virt_lines = true, virt_text_output = true, virt_lines_off_by_1 = true })
ui.sync(large, many)
collectgarbage("collect")
collectgarbage("stop")
local heap = collectgarbage("count")
local window_queries, get_window_info = 0, vim.fn.getwininfo
vim.fn.getwininfo = function(...)
  window_queries = window_queries + 1
  return get_window_info(...)
end
ui.refresh(large)
vim.fn.getwininfo = get_window_info
local allocated = collectgarbage("count") - heap
collectgarbage("restart")
assert(allocated < 1024, ("one refresh copied code spans per output: %.0f KiB allocated"):format(allocated))
assert(window_queries <= 3, "unchanged outputs must share window geometry within one refresh")
local output_marks = vim.api.nvim_buf_get_extmarks(large, vim.api.nvim_get_namespaces()["ipynb-native-output"], 0, -1, {})
assert(#output_marks == 100)
for index, mark in ipairs(output_marks) do
  assert(mark[2] == (index - 1) * 4 + 2, "every output must follow its own closing fence")
end
local width, rendered_widths = get_window_info(vim.api.nvim_get_current_win())[1].width, {}
local set_mark = vim.api.nvim_buf_set_extmark
vim.fn.getwininfo = function(...)
  local info = get_window_info(...)
  info[1].width = width
  return info
end
vim.api.nvim_buf_set_extmark = function(target, ns, row, col, opts)
  local id = set_mark(target, ns, row, col, opts)
  if target == large and opts.virt_lines then
    rendered_widths[row] = #opts.virt_lines[1][1][1]
    if row == 6 then width = width - 12 end
  end
  return id
end
many[2].lines = { "changed output" }
ui.sync(large, many)
vim.fn.getwininfo, vim.api.nvim_buf_set_extmark = get_window_info, set_mark
assert(rendered_widths[10] == rendered_widths[6] - 12,
  "render callbacks changing window geometry must invalidate it before the next output")
ui.refresh(large)
vim.api.nvim_win_set_cursor(0, { 398, 0 })
ui.refresh(large, true)
local position_queries, get_position = 0, vim.api.nvim_buf_get_extmark_by_id
vim.api.nvim_buf_get_extmark_by_id = function(...)
  position_queries = position_queries + 1
  return get_position(...)
end
for _ = 1, 5 do ui.refresh(large, true) end
vim.api.nvim_buf_get_extmark_by_id = get_position
assert(position_queries == 0, "unchanged cursor refresh must reuse source anchors")
vim.api.nvim_buf_set_lines(large, 0, 0, false, { "# inserted" })
ui.refresh(large, true)
local moved_marks = vim.api.nvim_buf_get_extmarks(large, vim.api.nvim_get_namespaces()["ipynb-native-output"], 0, -1, {})
for index, mark in ipairs(moved_marks) do
  assert(mark[2] == index * 4 - 1, "editing must invalidate cached anchors and move every output")
end
local shuffled = {}
for index = #many, 1, -1 do shuffled[#shuffled + 1] = many[index] end
local sync_position_queries, get_sync_position = 0, vim.api.nvim_buf_get_extmark_by_id
vim.api.nvim_buf_get_extmark_by_id = function(...)
  sync_position_queries = sync_position_queries + 1
  return get_sync_position(...)
end
ui.sync(large, shuffled)
vim.api.nvim_buf_get_extmark_by_id = get_sync_position
assert(sync_position_queries < #many * 10,
  "sync position reads must stay linear as cell count grows")
for index, synchronized in ipairs(ui.cells(large)) do
  assert(synchronized.id == index + 500 and synchronized.begin[1] == (index - 1) * 4 + 2,
    "sync must sort shuffled cells by their moved source anchors")
end
vim.api.nvim_win_set_cursor(0, { 399, 0 })
assert(ui.current(large).id == 600, "editing must preserve the selected cell identity")
ui.clear(large)
vim.api.nvim_buf_delete(large, { force = true })
local grid = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(grid)
local grid_win = vim.api.nvim_get_current_win()
ui.clear(buf)
ui.clear(other)
vim.api.nvim_buf_set_lines(grid, 0, -1, false, { "print('grid')", "", "", "" })
vim.b[grid].ipynb_code_spans = { { begin = { 0, 0 }, ["end"] = { 0, 13 } } }
vim.b[grid].ipynb_code_spans_tick = vim.api.nvim_buf_get_changedtick(grid)
ui.configure({ image_provider = "none", output_virt_lines = true, virt_text_output = true })
ui.sync(grid, { { id = "grid", begin = { 0, 0 }, ["end"] = { 1, 0 }, status = "done",
  success = true, execution_count = 1, lines = { "representative output", "second output line" } } })
local function visible_inspector()
  for _, win in ipairs(vim.api.nvim_list_wins()) do
    local config = vim.api.nvim_win_get_config(win)
    if config.relative ~= "" and not config.hide then return win, config end
  end
end
local function resize_grid(width, height)
  vim.o.columns, vim.o.lines = width, height
  vim.api.nvim_win_set_width(grid_win, width)
  vim.api.nvim_win_set_height(grid_win, math.max(1, height - 1))
end
local function check_grid()
  local win, config = visible_inspector()
  if win then
    local info = vim.fn.getwininfo(grid_win)[1]
    local gutter = info.textoff
    assert(config.col >= gutter and config.col + config.width + 2 <= info.width,
      ("inspector geometry must clear gutter and terminal bounds: col=%d width=%d gutter=%d columns=%d")
        :format(config.col, config.width, gutter, info.width))
    assert(config.width > 0 and config.height > 0 and config.row + config.height + 2 <= info.height,
      "the inspector must stay inside its source window")
    assert(config.row > vim.fn.screenpos(grid_win, 1, 1).row - info.winrow,
      "the inspector must start below the source code")
    local title = vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(win), 0, 1, false)[1]
    assert(vim.fn.strdisplaywidth(title) <= config.width, "the output label must fit its available width")
  end
  return win, config
end
local function has_inline_output()
  local ns = vim.api.nvim_get_namespaces()["ipynb-native-output"]
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(grid, ns, 0, -1, { details = true })) do
    if mark[4].virt_lines then return true end
  end
end
for _, width in ipairs({ 19, 20, 21, 80, 160 }) do
  resize_grid(width, 24)
  ui.hide()
  assert(ui.show("grid"), ("output must open at %dx24"):format(width))
  assert(check_grid(), ("output inspector must be visible at width %d"):format(width))
end
assert(ui.hide(true, "grid") and not visible_inspector(), "explicit hide must close the resized inspector")
assert(ui.show("grid") and check_grid(), "the inspector must restore after hide")
for _, height in ipairs({ 5, 6, 7 }) do
  resize_grid(80, height)
  ui.hide()
  ui.show("grid")
  local win = check_grid()
  if height == 5 then
    assert(not win and has_inline_output(), "a terminal below the border and gap minimum must fall back to inline output")
  end
end
resize_grid(12, 5)
ui.hide()
assert(not ui.show("grid") and not visible_inspector() and has_inline_output(),
  "a terminal below the border and gap minimum must fall back without a float")
resize_grid(80, 24)
ui.clear(grid)
local low_end = vim.api.nvim_win_get_height(grid_win) - 5
local low_lines = {}
for _ = 1, low_end + 3 do low_lines[#low_lines + 1] = "print(1)" end
vim.api.nvim_buf_set_lines(grid, 0, -1, false, low_lines)
vim.api.nvim_win_set_cursor(grid_win, { 1, 0 })
vim.b[grid].ipynb_code_spans = { { begin = { 0, 0 }, ["end"] = { low_end, 8 } } }
vim.b[grid].ipynb_code_spans_tick = vim.api.nvim_buf_get_changedtick(grid)
ui.sync(grid, { { id = "low", begin = { 0, 0 }, ["end"] = { low_end, 8 }, lines = traceback.lines } })
assert(ui.show("low"))
local low_win = assert(visible_inspector())
assert(vim.api.nvim_win_get_height(low_win) >= 3,
  "opening expanded output near the bottom must reveal more than its heading")
resize_grid(100, 40)
ui.clear(grid)
vim.api.nvim_buf_delete(grid, { force = true })
vim.api.nvim_buf_delete(buf, { force = true })
vim.api.nvim_buf_delete(other, { force = true })
print("pass: native output hide, image stability, status borders, and batch shutdown")
vim.cmd("qa!")
