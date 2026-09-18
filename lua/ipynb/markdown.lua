local M = {}

local namespace = vim.api.nvim_create_namespace("ipynb-markdown")

local bullet_cycle = { "●", "○", "■", "□", "◆", "◇" }

local language_names = {
  bash = "Bash",
  cpp = "C++",
  javascript = "JavaScript",
  js = "JavaScript",
  json = "JSON",
  julia = "Julia",
  lua = "Lua",
  markdown = "Markdown",
  md = "Markdown",
  python = "Python",
  py = "Python",
  rust = "Rust",
  sh = "Shell",
  sql = "SQL",
  toml = "TOML",
  ts = "TypeScript",
  typescript = "TypeScript",
  yaml = "YAML",
  yml = "YAML",
  zsh = "Zsh",
}

local syntax_groups = {
  "IpyNotebookCodeFence",
  "IpyNotebookFrontmatterFence",
  "IpyNotebookHeadingMarker",
  "IpyNotebookListMarker",
}

local function language_label(lang)
  local normalized = (lang or "text"):lower()
  local name = language_names[normalized] or normalized:gsub("^%l", string.upper)
  return string.format("  [%s]", name)
end

local function setup_highlights()
  local links = {
    CodeLabel = { "RenderMarkdownCodeInfo", "Comment" },
    CodeFenceLine = { "RenderMarkdownCode", "CursorLine" },
    CodeBlock = { "RenderMarkdownCode", "CursorLine" },
    ListBullet = { "RenderMarkdownBullet", "Special" },
    TaskUnchecked = { "DiagnosticWarn", "WarningMsg" },
    TaskChecked = { "DiagnosticOk", "String" },
    OrderedList = { "RenderMarkdownBullet", "Special" },
  }
  for level = 1, 6 do links["Heading" .. level] = { "RenderMarkdownH" .. level, "Title" } end
  for name, groups in pairs(links) do
    vim.api.nvim_set_hl(0, "IpyNotebook" .. name, {
      link = vim.fn.hlexists(groups[1]) == 1 and groups[1] or groups[2],
    })
  end
end

local highlight_group = vim.api.nvim_create_augroup("ipynb-markdown-highlights", { clear = true })
vim.api.nvim_create_autocmd("ColorScheme", { group = highlight_group, callback = setup_highlights })

local function setup_syntax(bufnr)
  vim.api.nvim_buf_call(bufnr, function()
    for _, group in ipairs(syntax_groups) do
      vim.cmd("silent! syntax clear " .. group)
    end

    vim.cmd([[syntax match IpyNotebookHeadingMarker /\v^\s*#+\s+/ conceal]])
    vim.cmd([[syntax match IpyNotebookCodeFence /\v^\s*```(\w+)?\s*$/ conceal]])
    vim.cmd([[syntax match IpyNotebookFrontmatterFence /^---$/ conceal]])
    vim.cmd([[syntax match IpyNotebookListMarker /\v^\s*(([-*+])|(\d+[.)]))\s+(\[[ xX-]\]\s+)?/ conceal]])
  end)
end

local function list_level(line)
  local indent = line:match("^(%s*)") or ""
  indent = indent:gsub("\t", "  ")
  return math.floor(#indent / 2)
end

local function bullet_symbol(level)
  return bullet_cycle[(level % #bullet_cycle) + 1]
end

local function list_overlay(line)
  local indent = line:match("^(%s*)") or ""
  local expanded_indent = indent:gsub("\t", "  ")
  local level = list_level(line)

  local ordered = line:match("^%s*(%d+[.)])%s+")
  if ordered then
    return #expanded_indent, ordered .. " ", "IpyNotebookOrderedList"
  end

  local task_state = line:match("^%s*[-*+]%s+%[([ xX-])%]%s+")
  if task_state then
    if task_state == "x" or task_state == "X" then
      return #expanded_indent, "☑ ", "IpyNotebookTaskChecked"
    end
    return #expanded_indent, "☐ ", "IpyNotebookTaskUnchecked"
  end

  if line:match("^%s*[-*+]%s+") then
    return #expanded_indent, bullet_symbol(level) .. " ", "IpyNotebookListBullet"
  end
end

local function clear(bufnr)
  vim.api.nvim_buf_clear_namespace(bufnr, namespace, 0, -1)
end

local function render(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end

  local changedtick = vim.api.nvim_buf_get_changedtick(bufnr)
  if vim.b[bufnr].ipynb_markdown_render_tick == changedtick then
    return
  end

  clear(bufnr)

  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local in_code_block = false
  local fence_row = nil

  for index, line in ipairs(lines) do
    local row = index - 1

    if not in_code_block then
      local hashes = line:match("^(#+)%s+")
      if hashes then
        local level = math.min(#hashes, 6)
        vim.api.nvim_buf_set_extmark(bufnr, namespace, row, 0, {
          line_hl_group = "IpyNotebookHeading" .. level,
          priority = 90,
        })
      end

      local col, overlay, group = list_overlay(line)
      if overlay then
        vim.api.nvim_buf_set_extmark(bufnr, namespace, row, col, {
          virt_text = { { overlay, group } },
          virt_text_pos = "overlay",
          hl_mode = "combine",
          priority = 110,
        })
      end
    end

    local language = line:match("^```%s*([%w_+-]+)")
    if not in_code_block and language then
      in_code_block = true
      fence_row = row
      vim.api.nvim_buf_set_extmark(bufnr, namespace, row, 0, {
        virt_text = { { language_label(language), "IpyNotebookCodeLabel" } },
        virt_text_pos = "overlay",
        hl_mode = "combine",
        line_hl_group = "IpyNotebookCodeFenceLine",
        priority = 120,
      })
    elseif line:match("^```%s*$") then
      if in_code_block and fence_row ~= nil then
        vim.api.nvim_buf_set_extmark(bufnr, namespace, row, 0, {
          line_hl_group = "IpyNotebookCodeFenceLine",
          priority = 100,
        })
      end
      in_code_block = not in_code_block
      fence_row = nil
    elseif in_code_block then
      vim.api.nvim_buf_set_extmark(bufnr, namespace, row, 0, {
        line_hl_group = "IpyNotebookCodeBlock",
        priority = 100,
      })
    end
  end
  vim.b[bufnr].ipynb_markdown_render_tick = changedtick
end

function M.enable(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end

  vim.wo.conceallevel = 3
  vim.wo.concealcursor = "nc"

  if vim.b[bufnr].ipynb_markdown_enabled ~= true then
    setup_highlights()
    setup_syntax(bufnr)
  end
  vim.b[bufnr].ipynb_markdown_enabled = true
  render(bufnr)
end

function M.refresh(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end

  if vim.b[bufnr].ipynb_markdown_enabled == false then
    return
  end

  render(bufnr)
end

function M.disable(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end

  clear(bufnr)
  vim.b[bufnr].ipynb_markdown_enabled = false
  vim.b[bufnr].ipynb_markdown_render_tick = nil
  vim.wo.conceallevel = 0
  vim.wo.concealcursor = ""
  vim.api.nvim_buf_call(bufnr, function()
    for _, group in ipairs(syntax_groups) do
      vim.cmd("silent! syntax clear " .. group)
    end
  end)
end

return M
