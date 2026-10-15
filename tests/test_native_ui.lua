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
local formatted = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_lines(formatted, 0, -1, false, { "```python", "print(1)", "```", "", "```python", "print(2)", "```" })
ui.sync(formatted, {
  { id = "format-first", begin = { 1, 0 }, ["end"] = { 1, 8 } },
  { id = "format-second", begin = { 5, 0 }, ["end"] = { 5, 8 } },
})
ui.with_positions(formatted, function()
  vim.api.nvim_buf_set_lines(formatted, 0, -1, false,
    { "```python", "print(1)", "# formatted", "```", "", "```python", "print(2)", "```" })
end)
local preserved = ui.positions(formatted)
assert(preserved[1].begin[1] == 1 and preserved[2].begin[1] == 6,
  "whole-buffer save hooks must preserve cell identities across inserted lines")
assert(preserved[1].source == "print(1)\n# formatted" and preserved[2].source == "print(2)")
assert(not pcall(ui.with_positions, formatted, function() error("save hook failed") end),
  "save hook errors must still abort saving")
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

local previous_snacks, previous_global_snacks = package.loaded.snacks, Snacks
local previous_image_style = package.loaded["ipynb.image_style"]
local snack_placements = {}
local snack_roundings = {}
Snacks = {
  image = {
    placement = { new = function(buffer, path, opts)
      local placement = { buffer = buffer, path = path, opts = opts, closed = 0 }
      function placement:close() self.closed = self.closed + 1 end
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
    if path == "/fallback.png" then callback(nil) else callback(path .. ".rounded") end
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
snacks_adapter.render(first_id)
assert(#snack_placements == 2 and snacks_adapter.image_size(first_id).width == 0,
  "rendering a cleared image ID must be a no-op")
snacks_adapter.clear_all()
assert(snack_placements[2].closed == 1, "clear_all must remove all remaining placements")
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
ui.refresh(large)
local allocated = collectgarbage("count") - heap
collectgarbage("restart")
assert(allocated < 1024, ("one refresh copied code spans per output: %.0f KiB allocated"):format(allocated))
local output_marks = vim.api.nvim_buf_get_extmarks(large, vim.api.nvim_get_namespaces()["ipynb-native-output"], 0, -1, {})
assert(#output_marks == 100)
for index, mark in ipairs(output_marks) do
  assert(mark[2] == (index - 1) * 4 + 2, "every output must follow its own closing fence")
end
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
resize_grid(100, 40)
ui.clear(grid)
vim.api.nvim_buf_delete(grid, { force = true })
vim.api.nvim_buf_delete(buf, { force = true })
vim.api.nvim_buf_delete(other, { force = true })
print("pass: native output hide, image stability, status borders, and batch shutdown")
vim.cmd("qa!")
