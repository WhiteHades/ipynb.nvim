local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
vim.opt.rtp:prepend(root)
local empty = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_lines(empty, 0, -1, false, {"```python", "```", "", "```python"})
local spans = require("ipynb.cells").spans(empty)
assert(#spans == 1 and vim.deep_equal(spans[1], {begin={1,0}, ["end"]={1,0}}),
  "closed bodyless cells need a zero-width span; an unclosed opener is not a cell")
vim.api.nvim_buf_delete(empty, {force=true})
package.preload["ipynb.molten_load_image_nvim"] = function()
  return { image_api = { panel_supported = function() return true end } }
end

local markdown = require("ipynb.markdown")
local namespace = vim.api.nvim_get_namespaces()["ipynb-markdown"]
local buffer = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(buffer)
vim.api.nvim_buf_set_name(buffer, vim.fn.tempname() .. ".ipynb")
vim.api.nvim_buf_set_lines(buffer, 0, -1, false, {
  "# Heading",
  "",
  "```python",
  "    print(1)",
  "```",
})
local cells, native = require("ipynb.cells"), require("ipynb.native")
local request, conversions = native.request, 0
native.request = function(method)
  assert(method == "code_spans")
  conversions = conversions + 1
  return cells.spans(buffer)
end
vim.b[buffer].ipynb_code_spans = cells.spans(buffer)
vim.b[buffer].ipynb_code_spans_tick = vim.api.nvim_buf_get_changedtick(buffer)
cells.executable(buffer)
local original = vim.api.nvim_buf_get_lines(buffer, 3, 4, false)[1]
for _ = 1, 60 do
  vim.api.nvim_buf_set_text(buffer, 3, 0, 3, 0, { "a" })
  assert(cells.executable(buffer)[1]["end"][2] == #original + _, "body edits must update the end byte column")
end
assert(conversions == 0, "ordinary code edits must not reconvert the notebook")
vim.api.nvim_buf_set_text(buffer, 3, 0, 3, 0, { "λ" })
assert(cells.executable(buffer)[1]["end"][2] == #original + 62, "cached end columns must count UTF-8 bytes")
vim.api.nvim_buf_set_text(buffer, 3, 0, 3, 2, { "" })
vim.api.nvim_buf_set_text(buffer, 3, 0, 3, 60, { "" })
assert(vim.deep_equal(cells.executable(buffer), cells.spans(buffer)), "deletion must keep executable ranges exact")
vim.api.nvim_buf_set_text(buffer, 2, 3, 2, 9, { "lua" })
cells.executable(buffer)
assert(conversions == 1, "editing a fence must invalidate authoritative cell membership")
cells.executable(buffer, true)
assert(conversions == 2, "forced execution must still consult the converter")
vim.api.nvim_buf_set_text(buffer, 2, 3, 2, 6, { "python" })
cells.executable(buffer)
native.request = request

local geometry = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_name(geometry, vim.fn.tempname() .. ".ipynb")
vim.api.nvim_buf_set_lines(geometry, 0, -1, false, { "```python", "", "```", "```python", "```" })
local geometry_conversions = 0
native.request = function()
  geometry_conversions = geometry_conversions + 1
  return cells.spans(geometry)
end
cells.executable(geometry)
local function edit_body(first, last, replacement)
  vim.api.nvim_buf_set_lines(geometry, first, last, false, replacement)
  assert(vim.deep_equal(cells.executable(geometry), cells.spans(geometry)), "body edits must retain exact cell geometry")
  assert(geometry_conversions == 1, "empty cells and newlines must retain executable membership")
end
edit_body(1, 2, { "x = 1" })
edit_body(1, 2, { "x = 1", "" })
edit_body(2, 3, { "y = 2" })
edit_body(2, 2, { "z = 3" })
edit_body(1, 4, {})
edit_body(1, 1, { "first = 1", "second = 2" })
edit_body(5, 5, { "third = 3" })
edit_body(5, 6, { "fourth = 4" })
vim.api.nvim_buf_set_lines(geometry, 7, 7, false, { "outside = 1" })
cells.executable(geometry)
assert(geometry_conversions == 2, "insertion after a closing fence must invalidate membership")
vim.api.nvim_buf_set_lines(geometry, 6, 7, false, { "not a fence" })
cells.executable(geometry)
assert(geometry_conversions == 3, "replacing a closing fence must invalidate membership")
native.request = request
vim.api.nvim_buf_delete(geometry, { force = true })

local display_options = { "conceallevel", "concealcursor", "foldexpr", "foldtext" }
local function settings(scope)
  local result = {}
  for _, name in ipairs(display_options) do
    result[name] = vim.api.nvim_get_option_value(name, { scope = scope })
  end
  return result
end
local global_display = settings("global")
local ordinary_display = settings("local")
markdown.enable(buffer)
assert(vim.deep_equal(settings("global"), global_display), "notebook display must not change editor defaults")
local ordinary = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_name(ordinary, vim.fn.tempname() .. ".md")
vim.api.nvim_buf_set_lines(ordinary, 0, -1, false, { "# Ordinary heading", "", "```python", 'color = "#ee4c3c"', "```" })
vim.api.nvim_set_current_buf(ordinary)
assert(vim.deep_equal(settings("local"), ordinary_display), "ordinary files must not inherit notebook display settings")
assert(#vim.api.nvim_buf_get_extmarks(ordinary, namespace, 0, -1, {}) == 0,
  "ordinary headings and hex colors must not receive notebook decorations")
vim.api.nvim_set_current_buf(buffer)
markdown.enable(buffer)
local notebook_window = vim.api.nvim_get_current_win()
local notebook_display = settings("local")
vim.cmd("vsplit")
vim.api.nvim_set_current_buf(ordinary)
assert(vim.deep_equal(settings("local"), ordinary_display), "a split must not leak notebook display settings")
vim.api.nvim_set_current_win(notebook_window)
assert(vim.deep_equal(settings("local"), notebook_display), "focus changes must preserve visible notebook rendering")
vim.cmd("only")
vim.api.nvim_buf_delete(ordinary, { force = true })
local first_tick = vim.api.nvim_buf_get_changedtick(buffer)
local first_marks = vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })
assert(#first_marks > 0)
local opening_border
for _, mark in ipairs(first_marks) do
  if mark[2] == 2 then opening_border = mark[4].virt_text end
end
assert(opening_border and opening_border[1][1]:find("╭", 1, true),
  "cell borders must move with native text even when terminal graphics are available")
local left_rail
for _, mark in ipairs(first_marks) do
  if mark[2] == 3 and mark[4].virt_text_pos == "win_col" then left_rail = mark[4] end
end
assert(left_rail and left_rail.virt_text[1][1] == "│" and left_rail.virt_text_win_col == 0
  and left_rail.priority == 300 and left_rail.hl_mode == "replace",
  "indented inactive code must keep its left border in window column zero")
assert(vim.b[buffer].ipynb_markdown_render_tick == first_tick)
local function surrounding_marks()
  local marks = {}
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })) do
    if mark[2] ~= 3 then marks[#marks + 1] = mark end
  end
  return marks
end
local surrounding = surrounding_marks()
for _ = 1, 12 do
  vim.api.nvim_buf_set_text(buffer, 3, 0, 3, 0, { "a" })
  markdown.refresh(buffer)
  assert(vim.deep_equal(surrounding_marks(), surrounding), "editing code must not rebuild surrounding blocks")
end
for _ = 1, 12 do vim.api.nvim_buf_set_text(buffer, 3, 0, 3, 0, { "a" }) end
markdown.refresh(buffer)
assert(vim.deep_equal(surrounding_marks(), surrounding), "coalesced key repeats must preserve surrounding blocks")
vim.api.nvim_buf_set_text(buffer, 3, 0, 3, 24, { "" })
markdown.refresh(buffer)
local incremental = vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })
vim.b[buffer].ipynb_markdown_render_tick = nil
markdown.refresh(buffer)
local fresh = vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })
for _, marks in ipairs({ incremental, fresh }) do for _, mark in ipairs(marks) do table.remove(mark, 1) end end
assert(vim.deep_equal(incremental, fresh), "incremental code decoration must match a full render")
first_marks = vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })
local dialect = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_name(dialect, vim.fn.tempname() .. ".ipynb")
vim.api.nvim_buf_set_lines(dialect, 0, -1, false, { "~~~python", "print(1)", "~~~" })
vim.b[dialect].ipynb_code_spans = cells.spans(dialect)
vim.b[dialect].ipynb_code_spans_tick = vim.api.nvim_buf_get_changedtick(dialect)
vim.api.nvim_set_current_buf(dialect)
markdown.enable(dialect)
local dialect_marks = vim.api.nvim_buf_get_extmarks(dialect, namespace, 0, -1, { details = true })
vim.api.nvim_buf_set_text(dialect, 1, 0, 1, 0, { "a" })
markdown.refresh(dialect)
assert(vim.deep_equal(dialect_marks, vim.api.nvim_buf_get_extmarks(dialect, namespace, 0, -1, { details = true })),
  "editing alternative fences must retain the full renderer's style")
markdown.disable(dialect)
vim.api.nvim_set_current_buf(buffer)
vim.api.nvim_buf_delete(dialect, { force = true })

markdown.enable(buffer)
local unchanged_marks = vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })
assert(vim.deep_equal(unchanged_marks, first_marks), "unchanged buffers should not be rendered twice")

vim.api.nvim_buf_set_lines(buffer, 0, 1, false, { "## Changed" })
markdown.refresh(buffer)
local changed_marks = vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })
assert(vim.b[buffer].ipynb_markdown_render_tick == vim.api.nvim_buf_get_changedtick(buffer))
assert(not vim.deep_equal(changed_marks, first_marks), "changed buffers should be rendered again")

local lines = {
  "```python", "first = 1", "```", "", "```python", "second = 2", "```",
}
vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines)
vim.b[buffer].ipynb_code_spans = cells.spans(buffer)
vim.b[buffer].ipynb_code_spans_tick = vim.api.nvim_buf_get_changedtick(buffer)
markdown.refresh(buffer)
local rendered_tick = vim.b[buffer].ipynb_markdown_render_tick
vim.api.nvim_buf_set_text(buffer, 1, 0, 1, 0, { "a" })
vim.api.nvim_buf_set_text(buffer, 5, 0, 5, 0, { "b" })
assert(vim.b[buffer].ipynb_code_edit.from_tick > rendered_tick, "edits to different rows must end the narrow refresh run")
markdown.refresh(buffer)
vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines)
markdown.refresh(buffer)
local cells = require("ipynb.cells")
local native_ui = require("ipynb.native_ui")
local cell_header = require("ipynb.cell_header")
local headers, screenpos = native_ui.cell_headers, vim.fn.screenpos
local animations, visibility_queries = vim.g.ipynb_animations, 0
native_ui.cell_headers = function()
  return { { row = 0, status = "done" }, { row = 4, status = "running" } }
end
vim.fn.screenpos = function(...)
  visibility_queries = visibility_queries + 1
  return screenpos(...)
end
vim.g.ipynb_animations = false
cell_header.refresh(buffer)
local header_marks = vim.api.nvim_buf_get_extmarks(buffer,
  vim.api.nvim_get_namespaces()["ipynb-cell-header"], 0, -1, { details = true })
assert(#header_marks == 2, "static headers must remain visible when animation is disabled")
assert(visibility_queries == 0, "static labels must not query every cell's screen position")
native_ui.cell_headers, vim.fn.screenpos, vim.g.ipynb_animations = headers, screenpos, animations
local original_current, original_ui_current, original_get_mode = cells.current, native_ui.current, vim.api.nvim_get_mode
local original_header_refresh, header_refreshes = cell_header.refresh, 0
cell_header.refresh = function() header_refreshes = header_refreshes + 1 end
local active = { first = 0, last = 3 }
cells.current = function() return active end
native_ui.current = function()
  if active.first == active.last then return { begin = { active.first, 0 }, ["end"] = { active.last, 0 } } end
  return { begin = { active.first + 1, 0 }, ["end"] = { active.last - 2, 0 } }
end
vim.api.nvim_get_mode = function() return { mode = "i" } end
markdown.refresh(buffer)
local function has_decoration(row)
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buffer, namespace, { row, 0 }, { row, -1 }, { details = true })) do
    if mark[4].virt_text or mark[4].line_hl_group then return true end
  end
  return false
end
assert(not has_decoration(0) and not has_decoration(1) and not has_decoration(2),
  "the active cell fence and body must remain unobscured while editing")
assert(has_decoration(4) and has_decoration(5) and has_decoration(6),
  "inactive cells should keep their normal borders")
active = { first = 0, last = 4 }
markdown.refresh(buffer)
assert(vim.b[buffer].ipynb_markdown_active_cell == "0:4",
  "a valid native cell range must replace the sticky range when inserting lines grows the cell")
active = { first = 0, last = 3 }
markdown.refresh(buffer)
local same_cell_refreshes = header_refreshes
markdown.refresh(buffer)
assert(header_refreshes == same_cell_refreshes, "cursor movement within one cell must not refresh its header")
active = { first = 4, last = 7 }
vim.api.nvim_win_set_cursor(0, { 5, 0 })
vim.api.nvim_exec_autocmds("CursorMovedI", { buffer = buffer })
assert(has_decoration(0) and has_decoration(1) and has_decoration(2),
  "moving insert focus must restore borders on the previous active cell")
assert(not has_decoration(4) and not has_decoration(5) and not has_decoration(6),
  "moving insert focus must reveal the newly active cell")
vim.api.nvim_buf_set_lines(buffer, 4, 5, false, { "```sql" })
markdown.refresh(buffer)
assert(not has_decoration(4) and vim.b[buffer].ipynb_markdown_render_tick == vim.api.nvim_buf_get_changedtick(buffer),
  "editing the fence language must keep the active cell raw and refresh the cache")
assert(header_refreshes == same_cell_refreshes + 1, "switching cells must refresh the native cell header")

native_ui.current, cells.current = function() return nil end, function() return nil end
vim.api.nvim_win_set_cursor(0, { 5, 0 })
for _, fence in ipairs({ "```sq", "```s", "```", "``", "`", "" }) do
  vim.api.nvim_buf_set_lines(buffer, 4, 5, false, { fence })
  if fence == "```sq" then
    vim.api.nvim_exec_autocmds("CursorMovedI", { buffer = buffer })
  else
    markdown.refresh(buffer)
  end
  assert(vim.b[buffer].ipynb_markdown_active_cell == "4:7" and vim.b[buffer].ipynb_markdown_syntax_state == "reveal:native",
    ("deleting opening fence characters must retain the active edit range and visible syntax (cell=%s, syntax=%s)")
      :format(vim.inspect(vim.b[buffer].ipynb_markdown_active_cell), vim.inspect(vim.b[buffer].ipynb_markdown_syntax_state)))
  assert(not has_decoration(4) and not has_decoration(5) and not has_decoration(6),
    "opening fence edits must not restore the active cell graphic")
end
vim.api.nvim_win_set_cursor(0, { 7, 0 })
for _, fence in ipairs({ "``", "`", "" }) do
  vim.api.nvim_buf_set_lines(buffer, 6, 7, false, { fence })
  markdown.refresh(buffer)
  assert(vim.b[buffer].ipynb_markdown_active_cell == "4:7" and vim.b[buffer].ipynb_markdown_syntax_state == "reveal:native",
    "deleting closing fence characters must retain the active edit range and visible syntax")
  assert(not has_decoration(4) and not has_decoration(5) and not has_decoration(6),
    "closing fence edits must not restore the active cell graphic")
end
vim.api.nvim_buf_set_lines(buffer, 4, 5, false, { "```sql" })
vim.api.nvim_buf_set_lines(buffer, 6, 7, false, { "```" })
native_ui.current = function() return { begin = { 5, 0 }, ["end"] = { 5, 0 } } end
markdown.refresh(buffer)
cells.current, native_ui.current, vim.api.nvim_get_mode = original_current, original_ui_current, original_get_mode
markdown.refresh(buffer)
assert(has_decoration(4) and vim.b[buffer].ipynb_markdown_syntax_state == "conceal:native",
  "leaving insert mode must restore the normal fence rendering")
cell_header.refresh = original_header_refresh

vim.api.nvim_buf_call(buffer, function() vim.cmd("syntax clear") end)
markdown.enable(buffer)
local concealed_fence = false
for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, {details=true})) do
  concealed_fence = concealed_fence or (mark[2] == 2 and mark[4].conceal == "")
end
assert(concealed_fence, "enable must restore fence concealment after syntax is cleared externally")

active = { first = 0, last = 0 }
native_ui.current = function() return { begin = { 0, 0 }, ["end"] = { 0, 0 } } end
cells.current = function() return { first = 0, last = 2 } end
vim.api.nvim_set_current_buf(buffer)
vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { "```python", "```" })
vim.api.nvim_get_mode = function() return { mode = "i" } end
markdown.refresh(buffer)
assert(not has_decoration(0) and not has_decoration(1), "empty cells must also expose both fences while editing")
cells.current, native_ui.current, vim.api.nvim_get_mode = original_current, original_ui_current, original_get_mode

vim.cmd("syntax enable")
vim.bo[buffer].filetype = "markdown"
vim.treesitter.start(buffer, "markdown")
local syntax_loads = 0
local syntax_probe = vim.api.nvim_create_autocmd("Syntax", { buffer = buffer, callback = function(ev)
  if ev.match == "markdown" then syntax_loads = syntax_loads + 1 end
end })
markdown.start_highlight(buffer)
assert(syntax_loads == 0 and vim.bo[buffer].syntax == "" and vim.b[buffer].ts_highlight,
  "replacing the notebook highlighter must not temporarily reload Vim syntax")
local query = vim.treesitter.highlighter.active[buffer]._queries.markdown._query
for _, pattern in pairs(query._processed_patterns) do
  for _, directive in ipairs(pattern.directives) do
    assert(directive[1] ~= "conceal", "Tree-sitter must not conceal fenced code markers")
  end
end
local highlighter_new = vim.treesitter.highlighter.new
local active_highlighter = vim.treesitter.highlighter.active[buffer]
markdown.start_highlight(0)
markdown.start_highlight()
assert(vim.treesitter.highlighter.active[buffer] == active_highlighter,
  "current-buffer aliases must reuse the notebook highlighter")
active_highlighter.ipynb_fences = nil
vim.treesitter.highlighter.new = function() error("simulated highlighter failure") end
local highlighted, highlight_error = pcall(markdown.start_highlight)
vim.treesitter.highlighter.new = highlighter_new
assert(not highlighted and tostring(highlight_error):find("simulated highlighter failure", 1, true),
  "failed highlighter construction must propagate its error")
assert(syntax_loads == 1 and vim.bo[buffer].syntax == "markdown" and not vim.b[buffer].ts_highlight,
  ("failed replacement must restore Vim syntax: loads=%d syntax=%s ts=%s")
    :format(syntax_loads, vim.bo[buffer].syntax, vim.inspect(vim.b[buffer].ts_highlight)))
vim.api.nvim_del_autocmd(syntax_probe)
markdown.start_highlight(buffer)

markdown.disable(buffer)
assert(#vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, {}) == 0)
assert(vim.b[buffer].ipynb_markdown_render_tick == nil)
markdown.enable(buffer)
assert(#vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, {}) > 0,
  "enabling after explicit disable should force a render")

vim.api.nvim_buf_delete(buffer, { force = true })

local metadata = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(metadata)
vim.api.nvim_buf_set_name(metadata, vim.fn.tempname() .. ".ipynb")
local metadata_lines = {
  "---", "jupyter:", "  kernelspec:", "    name: python3", "  custom:", "    keep: true", "---", "",
  "```python", "print(1)", "```",
}
vim.api.nvim_buf_set_lines(metadata, 0, -1, false, metadata_lines)
vim.b[metadata].ipynb_read_kernel = "python3"
vim.wo.foldmethod, vim.wo.foldenable, vim.wo.foldlevel, vim.wo.foldexpr = "expr", true, 0, "0"
local original_foldtext = vim.wo.foldtext
vim.wo.foldtext = ""
local metadata_display, metadata_global = settings("local"), settings("global")
markdown.enable(metadata)
assert(vim.deep_equal(settings("global"), metadata_global), "notebook metadata folds must not change editor defaults")
assert(vim.fn.foldclosed(1) == 1, "frontmatter should start folded into one summary row")
assert(vim.fn.foldtextresult(1):find("Kernel: python3", 1, true), "summary should use a short kernel label")
vim.cmd("silent! normal! zo")
assert(vim.fn.foldclosed(1) == -1, "raw metadata must remain deliberately expandable")
markdown.refresh(metadata)
assert(vim.fn.foldclosed(1) == -1, "refresh must preserve deliberate metadata expansion")
local concealed = vim.api.nvim_buf_call(metadata, function() return vim.fn.synconcealed(1, 1) end)
assert(concealed[1] == 0, "expanded frontmatter delimiters must remain visible and editable")
assert(vim.deep_equal(vim.api.nvim_buf_get_lines(metadata, 0, -1, false), metadata_lines),
  "compacting frontmatter must preserve unknown metadata text")
markdown.disable(metadata)
assert(vim.deep_equal(settings("local"), metadata_display), "disabling notebook rendering must restore window settings")
markdown.enable(metadata)
local metadata_window = vim.api.nvim_get_current_win()
vim.cmd("vsplit")
markdown.disable(metadata)
assert(vim.deep_equal(settings("local"), metadata_display), "disabling rendering must restore an inherited split")
vim.api.nvim_set_current_win(metadata_window)
assert(vim.deep_equal(settings("local"), metadata_display), "disabling rendering must restore the original window")
vim.cmd("only")
vim.wo.foldtext = original_foldtext
vim.api.nvim_buf_delete(metadata, { force = true })
local extended = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(extended)
vim.api.nvim_buf_set_lines(extended, 0, -1, false, {"````python", "literal = '''", "```", "'''", "````"})
markdown.enable(extended)
local literal, closing = false, false
for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(extended, namespace, 0, -1, {details=true})) do
  literal = literal or (mark[2] == 2 and mark[4].virt_text_pos == "inline")
  closing = closing or (mark[2] == 4 and mark[4].virt_text_pos == "overlay")
end
assert(literal and closing, "a shorter literal fence must stay inside the surrounding code block")
assert(vim.fn.synconcealed(3, 1)[1] == 0, "literal backticks inside code must remain visible")
if #vim.api.nvim_list_uis() > 0 then
  vim.wo.foldenable = false
  vim.api.nvim_win_set_cursor(0, {1, 0})
  vim.cmd("redraw!")
  local top = vim.fn.screenpos(vim.api.nvim_get_current_win(), 1, 1)
  local text = ""
  for col = 1, vim.o.columns do text = text .. vim.fn.screenstring(top.row, col) end
  assert(text:find("Python", 1, true), "concealing the fence must leave its rendered label visible")
  local body = vim.fn.screenpos(vim.api.nvim_get_current_win(), 3, 1)
  text = ""
  for col = 1, vim.o.columns do text = text .. vim.fn.screenstring(body.row, col) end
  assert(text:find("```", 1, true), "literal backticks must appear in the terminal grid")
end
markdown.disable(extended)
vim.api.nvim_buf_delete(extended, {force=true})
if #vim.api.nvim_list_uis() > 0 then
  local colors = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_set_current_buf(colors)
  vim.api.nvim_buf_set_lines(colors, 0, -1, false, { "```python", 'colors = "#000000 #ffffff #ee4c3c"', "```" })
  vim.wo.foldenable = false
  markdown.enable(colors)
  local color_ns = vim.api.nvim_create_namespace("test-color-provider")
  local samples = { { "#000000", 0xffffff, 0 }, { "#ffffff", 0, 0xffffff }, { "#ee4c3c", 0, 0xee4c3c } }
  local checks = 0
  for _, background in ipairs({ 0x181825, 0xf4ecd8, false }) do
    vim.api.nvim_set_hl(0, "Normal", { fg = 0x808080, bg = background or nil })
    vim.api.nvim_set_hl(0, "RenderMarkdownCode", { bg = background or nil })
    require("ipynb.highlights").setup()
    for _, priority in ipairs({ 200, 2000 }) do
      for _, sample in ipairs(samples) do
        vim.api.nvim_set_hl(0, "TestColorPreview", { fg = sample[2], bg = sample[3] })
        local col = assert(vim.api.nvim_buf_get_lines(colors, 1, 2, false)[1]:find(sample[1], 1, true)) - 1
        vim.api.nvim_buf_clear_namespace(colors, color_ns, 0, -1)
        vim.api.nvim_buf_set_extmark(colors, color_ns, 1, col, {
          end_col = col + #sample[1], hl_group = "TestColorPreview", priority = priority,
        })
        vim.cmd("redraw!")
        local pos = vim.fn.screenpos(vim.api.nvim_get_current_win(), 2, col + 1)
        local rendered = vim.api.nvim__inspect_cell(1, pos.row - 1, pos.col - 1)[2]
        assert(rendered.foreground == sample[2] and rendered.background == sample[3],
          "code fill must preserve the color provider's rendered foreground and background")
        checks = checks + 1
      end
    end
  end
  if vim.env.IPYNB_TEST_COLOR_REPORT then vim.fn.writefile({ tostring(checks) }, vim.env.IPYNB_TEST_COLOR_REPORT) end
  vim.api.nvim_buf_delete(colors, { force = true })
end
local original_snacks = rawget(_G, "Snacks")
local original_provider, original_extmark = vim.api.nvim_set_decoration_provider, vim.api.nvim_buf_set_extmark
local provider, scope_callback, rendered_scope, guide_marks, scope_stopped
_G.Snacks = {
  indent = {
    enabled = true,
    render_scope = function(scope, state) rendered_scope = { scope = scope, state = state } end,
    render_chunk = function(scope, state) rendered_scope = { scope = scope, state = state } end,
  },
  config = { get = function(_, defaults) return vim.deepcopy(defaults) end },
  scope = { attach = function(callback)
    scope_callback = callback
    return { update = function() end, disable = function() scope_stopped = true end }
  end },
  animate = { enabled = function() return false end },
  util = { redraw_range = function() end },
}
vim.api.nvim_set_decoration_provider = function(ns, value)
  if ns == vim.api.nvim_get_namespaces()["ipynb-code-guides"] then provider = value end
  return original_provider(ns, value)
end
local guide_ns = vim.api.nvim_create_namespace("ipynb-code-guides")
vim.api.nvim_buf_set_extmark = function(buf, ns, row, col, opts)
  if ns == guide_ns and opts.virt_text then
    guide_marks[#guide_marks + 1] = { row = row, col = opts.virt_text_win_col }
  end
  if opts.ephemeral then opts = vim.deepcopy(opts); opts.ephemeral = nil end
  return original_extmark(buf, ns, row, col, opts)
end
local guidebuf = vim.api.nvim_create_buf(false, true)
local pythonbuf = vim.api.nvim_create_buf(false, true)
vim.bo[pythonbuf].shiftwidth, vim.bo[pythonbuf].tabstop = 4, 4
package.loaded["otter.keeper"] = { rafts = { [guidebuf] = { buffers = { python = pythonbuf } } } }
guide_marks = {}
vim.api.nvim_set_current_buf(guidebuf)
vim.api.nvim_buf_set_name(guidebuf, vim.fn.tempname() .. ".ipynb")
vim.bo[guidebuf].buftype = ""
vim.bo[guidebuf].shiftwidth, vim.bo[guidebuf].tabstop = 2, 8
vim.api.nvim_buf_set_lines(guidebuf, 0, -1, false, { "```python3", "def load():", "\tif ready:", "\t\trun()", "```" })
markdown.enable(guidebuf)
assert(vim.b[guidebuf].snacks_indent == false, "notebook guides must not overlap Snacks' unshifted extmarks")
local guide_win = vim.api.nvim_get_current_win()
vim.api.nvim_win_set_cursor(guide_win, { 2, 0 })
scope_callback(guide_win, guidebuf, { buf = guidebuf, from = 2, to = 4, indent = 0 }, nil)
provider.on_win(nil, guide_win, guidebuf, 0, 4)
local shifted = false
for _, mark in ipairs(guide_marks) do shifted = shifted or (mark.row == 2 and mark.col == 2) end
assert(shifted, "notebook indent guides must align with the two-column code-cell inset")
assert(rendered_scope and rendered_scope.state.leftcol == -2,
  "the top-level function scope must use the same inset as the visible notebook code")
Snacks.indent.enabled = false
guide_marks = {}
provider.on_win(nil, guide_win, guidebuf, 0, 4)
vim.wait(10)
assert(#guide_marks == 0, "disabling Snacks guides must also disable notebook guides")
Snacks.indent.enabled = true
provider.on_win(nil, guide_win, guidebuf, 0, 4)
assert(#guide_marks > 0, "re-enabling Snacks guides must retain notebook alignment")
markdown.disable(guidebuf)
assert(vim.b[guidebuf].snacks_indent == nil, "disabling notebook rendering must restore the prior Snacks buffer option")
assert(scope_stopped, "closing the last notebook must stop its scope listener")
for _, extension in ipairs({ "py", "md" }) do
  local ordinary_buffer = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_name(ordinary_buffer, vim.fn.tempname() .. "." .. extension)
  vim.b[ordinary_buffer].ipynb_markdown_enabled = true
  require("ipynb.guides").enable(ordinary_buffer)
  assert(vim.b[ordinary_buffer].snacks_indent == nil, "notebook guide handling must not alter ordinary files")
  vim.api.nvim_buf_delete(ordinary_buffer, { force = true })
end
vim.api.nvim_buf_delete(guidebuf, { force = true })
vim.api.nvim_buf_delete(pythonbuf, { force = true })
vim.api.nvim_set_decoration_provider, vim.api.nvim_buf_set_extmark = original_provider, original_extmark
_G.Snacks = original_snacks
print("pass: markdown render caching, compact metadata, and re-enable behavior")
vim.cmd("qa!")
