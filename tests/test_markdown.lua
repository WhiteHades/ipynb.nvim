local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
vim.opt.rtp:prepend(root)
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
  "print(1)",
  "```",
})

markdown.enable(buffer)
local first_tick = vim.api.nvim_buf_get_changedtick(buffer)
local first_marks = vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })
assert(#first_marks > 0)
local opening_border
for _, mark in ipairs(first_marks) do
  if mark[2] == 2 then opening_border = mark[4].virt_text end
end
assert(opening_border and opening_border[1][1]:find("╭", 1, true),
  "cell borders must move with native text even when terminal graphics are available")
assert(vim.b[buffer].ipynb_markdown_render_tick == first_tick)

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
markdown.refresh(buffer)
local cells = require("ipynb.cells")
local native_ui = require("ipynb.native_ui")
local cell_header = require("ipynb.cell_header")
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

vim.api.nvim_buf_call(buffer, function() vim.cmd("syntax clear IpyNotebookCodeFence") end)
markdown.enable(buffer)
local syntax_listing = vim.api.nvim_buf_call(buffer, function() return vim.fn.execute("syntax list IpyNotebookCodeFence") end)
assert(syntax_listing:find("IpyNotebookCodeFence", 1, true),
  "enable must recover syntax groups cleared externally even when the render cache is current")

active = { first = 0, last = 0 }
native_ui.current = function() return { begin = { 0, 0 }, ["end"] = { 0, 0 } } end
cells.current = function() return { first = 0, last = 2 } end
vim.api.nvim_set_current_buf(buffer)
vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { "```python", "```" })
vim.api.nvim_get_mode = function() return { mode = "i" } end
markdown.refresh(buffer)
assert(not has_decoration(0) and not has_decoration(1), "empty cells must also expose both fences while editing")
cells.current, native_ui.current, vim.api.nvim_get_mode = original_current, original_ui_current, original_get_mode

markdown.start_highlight(buffer)
local query = vim.treesitter.highlighter.active[buffer]._queries.markdown._query
for _, pattern in pairs(query._processed_patterns) do
  for _, directive in ipairs(pattern.directives) do
    assert(directive[1] ~= "conceal", "Tree-sitter must not conceal fenced code markers")
  end
end

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
markdown.enable(metadata)
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
vim.wo.foldtext = original_foldtext
vim.api.nvim_buf_delete(metadata, { force = true })
print("pass: markdown render caching, compact metadata, and re-enable behavior")
vim.cmd("qa!")
