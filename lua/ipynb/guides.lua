local M = {}

local namespace = vim.api.nvim_create_namespace("ipynb-code-guides")
local buffers, scopes, config, scope_listener = {}, {}, nil, nil
local inset = 2
local group = vim.api.nvim_create_augroup("ipynb-code-guides", { clear = true })

local defaults = {
  indent = { enabled = true, priority = 1, char = "│", only_scope = false, only_current = false, hl = "SnacksIndent" },
  animate = { enabled = vim.fn.has("nvim-0.10") == 1, style = "out", easing = "linear", duration = { step = 20, total = 500 } },
  scope = { enabled = true, priority = 200, char = "│", underline = false, only_current = false, hl = "SnacksIndentScope" },
  chunk = { enabled = false, only_current = false, priority = 200, hl = "SnacksIndentChunk",
    char = { corner_top = "┌", corner_bottom = "└", horizontal = "─", vertical = "│", arrow = ">" } },
  filter = function(buf, win)
    return vim.g.snacks_indent ~= false and vim.b[buf].snacks_indent ~= false and vim.bo[buf].buftype == ""
  end,
}

local function code_rows(buf)
  local tick = vim.api.nvim_buf_get_changedtick(buf)
  local cached = buffers[buf].rows
  if cached and cached.tick == tick then return cached end

  local rows, languages, lines, fence = {}, {}, vim.api.nvim_buf_get_lines(buf, 0, -1, false), nil
  for row, line in ipairs(lines) do
    local ticks, language = line:match("^(```+)%s*([%w_+-]+)")
    local closing = line:match("^(```+)%s*$")
    if not fence and language then
      fence = { ticks = ticks, language = language:lower() }
    elseif fence then
      if closing and #closing >= #fence.ticks then
        fence = nil
      else
        rows[row] = true
        languages[row] = fence.language
      end
    end
  end
  cached = { tick = tick, rows = rows, languages = languages, lines = lines }
  buffers[buf].rows = cached
  return cached
end

local function cell_width(buf, language)
  local python = language == "py" or language:match("^python%d*$") or language:match("^py%d+$")
  if python then
    local ok, indent = pcall(require, "ipynb.indent")
    if ok and indent.python_options then
      local got, options = pcall(indent.python_options, buf)
      if got and options then
        local sw = options.shiftwidth == 0 and options.tabstop or options.shiftwidth
        if sw and sw > 0 then return sw, options.tabstop end
      end
    end
  end
  local ok, keeper = pcall(require, "otter.keeper")
  local embedded = ok and keeper.rafts[buf] and keeper.rafts[buf].buffers[language]
  if not embedded and python then
    embedded = ok and keeper.rafts[buf] and keeper.rafts[buf].buffers.python
  end
  if embedded and vim.api.nvim_buf_is_valid(embedded) then
    local sw, tabstop = vim.bo[embedded].shiftwidth, vim.bo[embedded].tabstop
    return sw == 0 and tabstop or sw, tabstop
  end
  local sw, tabstop = vim.bo[buf].shiftwidth, vim.bo[buf].tabstop
  return sw == 0 and tabstop or sw, tabstop
end

local function indentation(text, tabstop)
  local width = 0
  for char in (text:match("^%s*") or ""):gmatch(".") do
    width = char == "\t" and width + tabstop - width % tabstop or width + 1
  end
  return width
end

local function enabled(buf, win)
  local previous = buffers[buf].snacks_indent
  if vim.g.snacks_indent == false or previous == false or vim.bo[buf].buftype ~= "" then return false end
  local current = vim.b[buf].snacks_indent
  vim.b[buf].snacks_indent = previous
  local ok, result = pcall(config.filter, buf, win)
  vim.b[buf].snacks_indent = current
  return ok and result
end

local function scope_changed(win, buf, scope, previous)
  scopes[win] = scope
  if not vim.api.nvim_win_is_valid(win) then return end
  local redraw = Snacks.util.redraw_range
  if scope and Snacks.animate and Snacks.animate.add and Snacks.animate.enabled
    and config.animate.enabled and Snacks.animate.enabled({ buf = buf, name = "indent" }) then
    scope.win = win
    scope.animate = { from = scope.from, to = scope.from }
    local cursor = vim.api.nvim_win_get_cursor(win)
    local distance = scope.to - scope.from
    Snacks.animate.add(0, distance, function(value)
      if scopes[win] ~= scope then return end
      local style = config.animate.style
      local line = math.min(math.max(scope.from, cursor[1]), scope.to)
      if style == "down" then
        scope.animate = { from = scope.from, to = scope.from + value }
      elseif style == "up" then
        scope.animate = { from = scope.to - value, to = scope.to }
      elseif style == "out" or style == "up_down" then
        if style == "out" then
          scope.animate = { from = math.max(scope.from, line - value), to = math.min(scope.to, line + value) }
        else
          style = line - scope.from < scope.to - line and "down" or "up"
          scope.animate = style == "down" and { from = scope.from, to = scope.from + value }
            or { from = scope.to - value, to = scope.to }
        end
      end
      redraw(win, scope.animate.from, scope.animate.to)
    end, vim.tbl_extend("keep", { int = true, id = "ipynb_indent_scope_" .. win, buf = buf }, config.animate))
  elseif scope then
    redraw(win, scope.from, scope.to)
  end
  if previous then redraw(win, previous.from, previous.to) end
end

local function attach_scope()
  if scope_listener then return end
  local scope_opts = vim.deepcopy(config.scope)
  local user_filter = scope_opts.filter
  scope_opts.filter = function(buf)
    return Snacks.indent.enabled and vim.g.snacks_indent ~= false
      and buffers[buf] ~= nil and vim.bo[buf].buftype == ""
      and vim.g.snacks_scope ~= false and vim.b[buf].snacks_scope ~= false
      and (not user_filter or user_filter(buf))
  end
  scope_listener = Snacks.scope.attach(scope_changed, scope_opts)
end

local function hl(level, value)
  return type(value) == "string" and value or value[(level - 1) % #value + 1]
end

local function render(win, buf, top, bottom)
  if not buffers[buf] or not Snacks.indent.enabled or not enabled(buf, win) then return end
  local cfg, data = config, code_rows(buf)
  local widths = {}
  local function width(language)
    if not widths[language] then widths[language] = { cell_width(buf, language) } end
    return widths[language][1], widths[language][2]
  end
  local current = win == vim.api.nvim_get_current_win()
  local show_indent = cfg.indent.enabled and (not cfg.indent.only_current or current)
  local show_scope = cfg.scope.enabled and (not cfg.scope.only_current or current)
  local show_chunk = cfg.chunk.enabled and (not cfg.chunk.only_current or current)
  if not show_indent and not show_scope and not show_chunk then return end
  local view = vim.api.nvim_win_call(win, vim.fn.winsaveview)
  local cursor_line = vim.api.nvim_win_get_cursor(win)[1]
  local sw = width(data.languages[cursor_line] or "")
  local state = {
    win = win, buf = buf, top = top + 1, bottom = bottom + 1,
    leftcol = view.leftcol - inset, shiftwidth = sw,
    indents = {}, blanks = {}, indent_offset = 0,
    breakindent = vim.wo[win].breakindent and vim.wo[win].wrap,
  }
  local active_first, active_last = vim.b[buf].ipynb_markdown_active_first, vim.b[buf].ipynb_markdown_active_last
  local scope = scopes[win]
  if not data.rows[cursor_line] then
    scope = nil
  elseif scope then
    local first, last = cursor_line, cursor_line
    while data.rows[first - 1] do first = first - 1 end
    while data.rows[last + 1] do last = last + 1 end
    scope = vim.tbl_extend("force", {}, scope, { from = math.max(scope.from, first), to = math.min(scope.to, last) })
    if scope.from > scope.to then scope = nil end
  end
  local guide_line = scope and scope.from or cursor_line
  if type(active_first) == "number" and type(active_last) == "number"
    and guide_line - 1 >= active_first and guide_line - 1 < active_last then state.leftcol = view.leftcol end
  if cfg.indent.only_scope then
    if not scope then return end
    state.indent_offset = scope.indent or 0
    state.top = math.max(state.top, scope.from)
    state.bottom = math.min(state.bottom, scope.to)
  end
  if state.top > state.bottom then return end
  local previous_indent
  for line = state.top, state.bottom do
    if data.rows[line] then
      local line_sw, tabstop = width(data.languages[line] or "")
      local value = indentation(data.lines[line] or "", tabstop)
      local text = data.lines[line] or ""
      if text:match("^%s*$") then
        local before, after
        for row = line - 1, 1, -1 do
          if not data.rows[row] then break end
          if not (data.lines[row] or ""):match("^%s*$") then before = indentation(data.lines[row], tabstop); break end
        end
        for row = line + 1, #data.lines do
          if not data.rows[row] then break end
          if not (data.lines[row] or ""):match("^%s*$") then after = indentation(data.lines[row], tabstop); break end
        end
        if before and after then
          value = math.min(before, after)
          if before ~= after and value > 0 then value = value + line_sw end
        end
        state.blanks[line] = true
      end
      state.indents[line] = value
      local row_in_active_cell = type(active_first) == "number" and type(active_last) == "number"
        and line - 1 >= active_first and line - 1 < active_last
      if show_indent and not (cfg.indent.only_scope and not scope) then
        local level = math.floor(value / line_sw)
        local parent = previous_indent or value
        level = math.min(level, math.floor((parent + line_sw) / line_sw))
        local offset = cfg.indent.only_scope and math.max(math.floor(state.indent_offset / line_sw), 0) or 0
        for index = 1 + offset, level do
          local col = (row_in_active_cell and 0 or inset) + (index - 1) * line_sw - view.leftcol
          if col >= 0 then
            local guide_hl = hl(index, cfg.indent.hl)
            vim.api.nvim_buf_set_extmark(buf, namespace, line - 1, 0, {
              virt_text = { { cfg.indent.char, guide_hl } }, virt_text_pos = "overlay", virt_text_win_col = col,
              hl_mode = "combine", priority = cfg.indent.priority,
              virt_text_repeat_linebreak = state.breakindent or nil, ephemeral = true,
            })
          end
        end
      end
      if text:match("^%s*$") then
        previous_indent = nil
      else
        previous_indent = value
      end
    else
      previous_indent = nil
    end
  end

  if not scope then return end
  if scope.animate then
    scope.animate.from = math.max(scope.animate.from, scope.from)
    scope.animate.to = math.min(scope.animate.to, scope.to)
  end
  if scope.from > scope.to then return end
  if show_chunk and (scope.indent or 0) >= sw then
    Snacks.indent.render_chunk(scope, state)
  elseif show_scope then
    Snacks.indent.render_scope(scope, state)
  end
end

local function setup()
  if config then return true end
  if not Snacks or not Snacks.indent or not Snacks.scope or not Snacks.config or not Snacks.indent.enabled then return false end
  if type(Snacks.indent.render_scope) ~= "function" or type(Snacks.indent.render_chunk) ~= "function"
    or type(Snacks.scope.attach) ~= "function" then return false end
  local next_config = Snacks.config.get("indent", defaults)
  config = next_config
  vim.api.nvim_set_decoration_provider(namespace, { on_win = function(_, win, buf, top, bottom) render(win, buf, top, bottom) end })
  if config.scope.enabled or config.chunk.enabled or config.indent.only_scope then attach_scope() end
  return true
end

local function try_enable(buf)
  if vim.api.nvim_buf_is_valid(buf) and vim.b[buf].ipynb_markdown_enabled == true then M.enable(buf) end
end

vim.api.nvim_create_autocmd("User", { group = group, pattern = "VeryLazy", callback = function()
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do try_enable(buf) end
end })
vim.api.nvim_create_autocmd("BufEnter", { group = group, callback = function(ev) try_enable(ev.buf) end })
vim.api.nvim_create_autocmd({ "BufUnload", "BufWipeout" }, { group = group, callback = function(ev) M.disable(ev.buf) end })
vim.api.nvim_create_autocmd("WinClosed", { group = group, callback = function(ev)
  local win = tonumber(ev.match)
  scopes[win] = nil
  if Snacks and Snacks.animate and Snacks.animate.del then Snacks.animate.del("ipynb_indent_scope_" .. win) end
end })

function M.enable(buf)
  if not vim.api.nvim_buf_is_valid(buf) or buffers[buf] or vim.b[buf].ipynb_markdown_enabled ~= true
    or vim.fn.fnamemodify(vim.api.nvim_buf_get_name(buf), ":e") ~= "ipynb" then return end
  if vim.b[buf].snacks_indent == false or not setup() then return end
  buffers[buf] = { snacks_indent = vim.b[buf].snacks_indent }
  vim.b[buf].snacks_indent = false
  if scope_listener then scope_listener:update(vim.fn.win_findbuf(buf), { now = true }) end
end

function M.disable(buf)
  local saved = buffers[buf]
  if not saved then return end
  if vim.api.nvim_buf_is_valid(buf) then
    vim.api.nvim_buf_clear_namespace(buf, namespace, 0, -1)
    if vim.b[buf].snacks_indent == false then vim.b[buf].snacks_indent = saved.snacks_indent end
  end
  buffers[buf] = nil
  for win, scope in pairs(scopes) do if scope and scope.buf == buf then scopes[win] = nil end end
  if scope_listener then
    if next(buffers) then
      scope_listener:update(vim.api.nvim_list_wins(), { now = true })
    else
      scope_listener:disable()
      scope_listener, config = nil, nil
    end
  end
end

return M
