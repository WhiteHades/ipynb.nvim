local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
vim.opt.rtp:prepend(root)

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
assert(vim.b[buffer].ipynb_markdown_render_tick == first_tick)

markdown.enable(buffer)
local unchanged_marks = vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })
assert(vim.deep_equal(unchanged_marks, first_marks), "unchanged buffers should not be rendered twice")

vim.api.nvim_buf_set_lines(buffer, 0, 1, false, { "## Changed" })
markdown.refresh(buffer)
local changed_marks = vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, { details = true })
assert(vim.b[buffer].ipynb_markdown_render_tick == vim.api.nvim_buf_get_changedtick(buffer))
assert(not vim.deep_equal(changed_marks, first_marks), "changed buffers should be rendered again")

markdown.disable(buffer)
assert(#vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, {}) == 0)
assert(vim.b[buffer].ipynb_markdown_render_tick == nil)
markdown.enable(buffer)
assert(#vim.api.nvim_buf_get_extmarks(buffer, namespace, 0, -1, {}) > 0,
  "enabling after insert-mode disable should force a render")

vim.api.nvim_buf_delete(buffer, { force = true })
print("pass: markdown render caching and re-enable behavior")
vim.cmd("qa!")
