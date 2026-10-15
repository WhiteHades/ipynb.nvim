local M = {}

local namespace = vim.api.nvim_create_namespace("ipynb-markdown")
local highlight_query

function M.start_highlight(buf)
  local active = vim.treesitter.highlighter.active[buf]
  if active and active.ipynb_fences then return end
  if not highlight_query then
    local parts = {}
    for _, path in ipairs(vim.treesitter.query.get_files("markdown", "highlights")) do
      parts[#parts + 1] = table.concat(vim.fn.readfile(path), "\n")
    end
    -- Concealed fence rows also hide their borders and anchored virtual output.
    highlight_query = table.concat(parts, "\n"):gsub('%(%#set!%s+conceal_lines%s+""%)', "")
  end
  local parser = assert(vim.treesitter.get_parser(buf, "markdown"))
  vim.treesitter.stop(buf)
  local highlighter = vim.treesitter.highlighter.new(parser, { queries = { markdown = highlight_query } })
  highlighter.ipynb_fences = true
end

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
  return " " .. name .. " "
end

local function text_width(bufnr)
  local width
  for _, win in ipairs(vim.fn.win_findbuf(bufnr)) do
    local info = vim.fn.getwininfo(win)[1]
    local available = math.max(2, info.width - info.textoff)
    width = width and math.min(width, available) or available
  end
  return width or math.max(2, vim.o.columns)
end

local function border(left, right, label, width)
  local text = left .. label
  while vim.fn.strdisplaywidth(text) > width - 1 do
    text = vim.fn.strcharpart(text, 0, vim.fn.strchars(text) - 1)
  end
  return {
    { text, label == "" and "IpyNotebookCodeBorder" or "IpyNotebookCodeLabel" },
    { string.rep("─", width - 1 - vim.fn.strdisplaywidth(text)) .. right, "IpyNotebookCodeBorder" },
  }
end

local function setup_highlights()
  local links = {
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

    if not require("ipynb.markdown_integration").active() then
      vim.cmd([[syntax match IpyNotebookHeadingMarker /\v^\s*#+\s+/ conceal]])
      vim.cmd([[syntax match IpyNotebookListMarker /\v^\s*(([-*+])|(\d+[.)]))\s+(\[[ xX-]\]\s+)?/ conceal]])
    end
    vim.cmd([[syntax match IpyNotebookCodeFence /\v^\s*```(\w+)?\s*$/ conceal]])
    vim.cmd([[syntax match IpyNotebookFrontmatterFence /^---$/ conceal]])
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

function M.foldexpr(original)
  local last = vim.b.ipynb_frontmatter_end
  if last and vim.v.lnum <= last then
    if vim.v.lnum == 1 then return ">1" end
    return vim.v.lnum == last and "<1" or 1
  end
  return vim.api.nvim_eval(original)
end

function M.foldtext(original)
  if vim.v.foldstart == 1 and vim.v.foldend == vim.b.ipynb_frontmatter_end then
    return vim.b.ipynb_metadata_label
  end
  return vim.api.nvim_eval(original)
end

local function metadata_fold(bufnr, close)
  local last = vim.b[bufnr].ipynb_frontmatter_end
  if not last then return end
  if close and vim.api.nvim_get_current_buf() == bufnr
    and vim.api.nvim_get_mode().mode:match("^[iR]") and vim.api.nvim_win_get_cursor(0)[1] <= last then
    close = false
  end
  for _, win in ipairs(vim.fn.win_findbuf(bufnr)) do
    vim.api.nvim_win_call(win, function()
      if not vim.wo.foldenable then return end
      local method = vim.wo.foldmethod
      if method ~= "expr" and method ~= "manual" then return end
      local expression = "v:lua.require'ipynb.markdown'.foldexpr("
      if method == "expr" and vim.wo.foldexpr:sub(1, #expression) ~= expression then
        vim.wo.foldexpr = expression .. vim.fn.string(vim.wo.foldexpr) .. ")"
      elseif vim.wo.foldmethod == "manual" and vim.fn.foldlevel(1) == 0 then
        vim.cmd(("silent 1,%dfold"):format(last))
      end
      -- Keep other fold providers untouched; unsupported providers show raw metadata.
      if vim.fn.foldlevel(1) == 0 then return end
      local text = "v:lua.require'ipynb.markdown'.foldtext("
      if vim.wo.foldtext ~= "" and vim.wo.foldtext:sub(1, #text) ~= text then
        vim.wo.foldtext = text .. vim.fn.string(vim.wo.foldtext) .. ")"
      end
      if close then vim.cmd("silent! 1foldclose") else vim.cmd("silent! 1foldopen") end
    end)
  end
end

local function render(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end

  local changedtick = vim.api.nvim_buf_get_changedtick(bufnr)
  local width = text_width(bufnr)
  if vim.b[bufnr].ipynb_markdown_render_tick == changedtick and vim.b[bufnr].ipynb_markdown_width == width then
    return
  end
  if vim.b[bufnr].ipynb_markdown_render_tick ~= changedtick and not vim.api.nvim_get_mode().mode:match("^[iR]") then
    pcall(require("ipynb.cells").executable, bufnr)
  end

  clear(bufnr)

  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local frontmatter_end
  if lines[1] == "---" and lines[2] == "jupyter:" then
    for index = 3, #lines do
      if lines[index] == "---" then frontmatter_end = index; break end
    end
  end
  vim.b[bufnr].ipynb_frontmatter_end = frontmatter_end
  if frontmatter_end then
    local kernel = vim.b[bufnr].ipynb_read_kernel
    local label = type(kernel) == "string" and ("Notebook kernel: " .. kernel:gsub("[\r\n]", " ")) or "Notebook metadata"
    vim.b[bufnr].ipynb_metadata_label = label
    vim.api.nvim_buf_set_extmark(bufnr, namespace, 0, 0, {
      virt_text = { { label, "IpyNotebookCodeLabel" } }, virt_text_pos = "overlay", priority = 120,
    })
  end
  local in_code_block = false
  local fence_row = nil
  local panels = require("ipynb.panels")
  local graphical = panels.available()
  local ranges, fence_label = {}, nil

  for index, line in ipairs(lines) do
    local row = index - 1

    if not require("ipynb.markdown_integration").active() and index > (frontmatter_end or 0) and not in_code_block then
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
      fence_label = language_label(language)
      vim.api.nvim_buf_set_extmark(bufnr, namespace, row, 0, {
        virt_text = graphical and { { "   " .. fence_label, "IpyNotebookCodeLabel" },
          { string.rep(" ", math.max(0, width - 3 - vim.fn.strdisplaywidth(fence_label))), "IpyNotebookCodeBlock" } }
          or border("╭─", "╮", fence_label, width),
        virt_text_pos = "overlay",
        hl_mode = "combine",
        line_hl_group = "IpyNotebookCodeBlock",
        priority = 300,
      })
    elseif line:match("^```%s*$") then
      if in_code_block and fence_row ~= nil then
        ranges[#ranges + 1] = { first = fence_row, last = row, width = width,
          label = vim.fn.strdisplaywidth(fence_label) + 1 }
        vim.api.nvim_buf_set_extmark(bufnr, namespace, row, 0, {
          virt_text = graphical and { { string.rep(" ", width), "IpyNotebookCodeBlock" } } or border("╰", "╯", "", width),
          virt_text_pos = "overlay",
          line_hl_group = "IpyNotebookCodeBlock",
          priority = 300,
        })
      end
      in_code_block = not in_code_block
      fence_row = nil
    elseif in_code_block then
      vim.api.nvim_buf_set_extmark(bufnr, namespace, row, 0, {
        line_hl_group = "IpyNotebookCodeBlock",
        virt_text = { { graphical and "  " or "│ ", "IpyNotebookCodeBorder" } },
        virt_text_pos = "inline",
        virt_text_repeat_linebreak = true,
        hl_mode = "combine",
        priority = 300,
      })
      vim.api.nvim_buf_set_extmark(bufnr, namespace, row, #line, {
        -- Keep long source lines visible rather than overlaying a right-hand rail.
        virt_text = { { graphical and "  " or " │", "IpyNotebookCodeBorder" } },
        virt_text_pos = "eol_right_align",
        hl_mode = "combine",
        priority = 300,
      })
    end
  end
  panels.code(bufnr, ranges)
  vim.b[bufnr].ipynb_markdown_render_tick = changedtick
  vim.b[bufnr].ipynb_markdown_width = width
end

function M.enable(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end
  for _, win in ipairs(vim.fn.win_findbuf(bufnr)) do
    vim.wo[win].conceallevel = 3
    vim.wo[win].concealcursor = "nic"
  end

  if vim.b[bufnr].ipynb_markdown_enabled ~= true then
    setup_highlights()
  end
  setup_syntax(bufnr)
  vim.b[bufnr].ipynb_markdown_enabled = true
  render(bufnr)
  metadata_fold(bufnr, true)
  require("ipynb.cell_header").refresh(bufnr)
end

function M.refresh(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end

  if vim.b[bufnr].ipynb_markdown_enabled == false then
    return
  end

  render(bufnr)
  metadata_fold(bufnr, true)
end

function M.disable(bufnr)
  if not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end

  clear(bufnr)
  require("ipynb.panels").code(bufnr, {})
  metadata_fold(bufnr, false)
  require("ipynb.cell_header").clear(bufnr)
  vim.b[bufnr].ipynb_markdown_enabled = false
  vim.b[bufnr].ipynb_markdown_render_tick = nil
  for _, win in ipairs(vim.fn.win_findbuf(bufnr)) do
    vim.wo[win].conceallevel = 0
    vim.wo[win].concealcursor = ""
  end
  vim.api.nvim_buf_call(bufnr, function()
    for _, group in ipairs(syntax_groups) do
      vim.cmd("silent! syntax clear " .. group)
    end
  end)
end

return M
