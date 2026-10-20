use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

#[test]
#[ignore = "requires installed Neovim plugins and a helper with ipykernel"]
fn notebook_opens_rendered_and_runs_from_its_cell_label() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let base = root.join(".tmp/notebook-startup");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::tempdir_in(base).unwrap();
    let python = std::env::var("IPYNB_TEST_PYTHON").unwrap();
    std::os::unix::fs::symlink(
        Path::new(&python).parent().unwrap().parent().unwrap(),
        fixture.path().join(".venv"),
    )
    .unwrap();
    let path = fixture.path().join("view.ipynb");
    let original = serde_json::to_vec(&json!({"nbformat":4,"nbformat_minor":5,
        "metadata":{"kernelspec":{"name":"python3","display_name":"Python 3","language":"python"},"keep":"metadata"},
        "cells":[{"id":"prose","cell_type":"markdown","metadata":{},"source":"# Heading\n\n```bash\nraise RuntimeError('prose must not run')\n```"},
          {"id":"run","cell_type":"code","metadata":{},"source":"print(42)\n","execution_count":null,"outputs":[]},
          {"id":"next","cell_type":"code","metadata":{},"source":"print(43)","execution_count":null,"outputs":[]}]
    })).unwrap();
    fs::write(&path, &original).unwrap();
    let report = fixture.path().join("report.json");
    let script = fixture.path().join("view.lua");
    fs::write(&script, r#"
local result = {}
local ok, err = xpcall(function()
  if _G.LazyVim and not vim.g.did_very_lazy then
    vim.g.did_very_lazy = true
    vim.api.nvim_exec_autocmds('User', {pattern='VeryLazy', modeline=false})
  end
  vim.wait(1000)
  local buf = vim.api.nvim_get_current_buf()
  local indent = require('ipynb.indent')
  assert(vim.b[buf].ipynb_python_indent_attached, 'notebook Python indent adapter did not attach')
  local python_options = indent.python_options(buf)
  assert(python_options.shiftwidth > 0, 'Python settings must use an effective indentation width')
  local indent_buf = vim.api.nvim_create_buf(false, true)
  local keeper = require('otter.keeper')
  local embedded = assert(keeper.rafts[buf].buffers.python)
  keeper.rafts[indent_buf] = {buffers={python=embedded}}
  vim.api.nvim_buf_set_lines(indent_buf, 0, -1, false, {
    '```python', 'def load():', '    if ready:', '        Path("x").mkdir()', '', '```',
    '', '```python', 'def nested():', '    with open("x") as stream:',
    '        result = calculate(', '            stream,', '            value,',
    '        )', '        return result', '```',
  })
  vim.bo[indent_buf].filetype = 'markdown'
  vim.b[indent_buf].notebook_markdown = true
  vim.bo[indent_buf].shiftwidth, vim.bo[indent_buf].tabstop = 2, 2
  vim.bo[indent_buf].expandtab = true
  local markdown_comments = {vim.bo[indent_buf].comments, vim.bo[indent_buf].commentstring,
    vim.bo[indent_buf].formatoptions}
  indent.attach(indent_buf)
  vim.api.nvim_set_current_buf(indent_buf)
  vim.api.nvim_win_set_cursor(0, {4, 0})
  vim.api.nvim_exec_autocmds('CursorMoved', {buffer=indent_buf})
  assert(vim.bo[indent_buf].shiftwidth == python_options.shiftwidth
    and vim.bo[indent_buf].tabstop == python_options.tabstop
    and vim.bo[indent_buf].expandtab == python_options.expandtab,
    'Python cell indentation did not use the embedded Python settings')
  local python_indent = require('nvim-treesitter.indent').get_indent
  assert(python_indent(5) == 8, 'Enter after an 8-space Python statement must retain 8-space indentation')
  local nested_indents = {python_indent(3), python_indent(4), python_indent(5),
    python_indent(10), python_indent(11), python_indent(12), python_indent(13),
    python_indent(14), python_indent(15)}
  assert(vim.deep_equal(nested_indents, {4, 8, 8, 4, 8, 12, 12, 8, 8}),
    'if, with, multiline, and return indentation must follow Python syntax: ' .. vim.inspect(nested_indents))
  local function replay(filetype, lines, keys, paste)
    local target = vim.api.nvim_create_buf(false, true)
    local source = filetype == 'markdown' and vim.list_extend({'```python'}, vim.deepcopy(lines)) or lines
    if filetype == 'markdown' then source[#source + 1] = '```' end
    vim.api.nvim_buf_set_lines(target, 0, -1, false, source)
    vim.bo[target].filetype = filetype
    if filetype == 'markdown' then
      vim.b[target].notebook_markdown = true
      keeper.rafts[target] = {buffers={python=embedded}}
      vim.bo[target].shiftwidth = python_options.shiftwidth
      vim.bo[target].tabstop = python_options.tabstop
      vim.bo[target].expandtab = python_options.expandtab
      indent.attach(target)
    end
    vim.api.nvim_set_current_buf(target)
    vim.api.nvim_win_set_cursor(0, {filetype == 'markdown' and #lines + 1 or #lines, #lines[#lines]})
    vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(keys, true, false, true), 'mtx', false)
    if paste then assert(vim.api.nvim_paste(paste, false, -1)) end
    local result = vim.api.nvim_buf_get_lines(target, 0, -1, false)
    if filetype == 'markdown' then
      table.remove(result, #result)
      table.remove(result, 1)
      keeper.rafts[target] = nil
    end
    vim.api.nvim_buf_delete(target, {force=true})
    return result
  end
  local replays = {
    {'comment continuation', {'def f():', '    value = 1', '    # note:'}, 'A<CR>next<Esc>'},
    {'single function', {'def f():'}, 'A<CR>return 1<Esc>'},
    {'typed signature', {'def f(', '    value: int,', '    name: str,'}, 'A<CR>) -> bool:<CR>return True<Esc>'},
    {'nested if', {'def f():', '    if ready:', '        value = 1'}, 'A<CR>return value<Esc>'},
    {'bracket continuation', {'values = [', '    1,'}, 'A<CR>2,<CR>]<Esc>'},
    {'double newline', {'def f():'}, 'A<CR><CR>pass<Esc>'},
    {'intentional Ctrl-D dedent', {'def f():', '    return 1'}, 'A<CR><C-D>pass<Esc>'},
    {'closed triple quote', {'text = """done"""'}, 'A<CR>next = 1<Esc>'},
    {'unfinished dictionary', {'values = {', '    "a": 1,'}, 'A<CR>"b": 2,<Esc>'},
    {'nested return', {'def f():', '    if ready:', '        return 1'}, 'A<CR>return 2<Esc>'},
    {'return argument', {'def f():', '    return call(', '        1,', '    )'},
      'kA<CR>2,<Esc>'},
    {'completion acceptance', {'def f():'},
      'A<CR><Cmd>lua vim.fn.complete(5, {"known_name"})<CR><C-n><C-y><CR>next_value = 2<Esc>'},
    {'backslash continuation', {'value = first + \\'}, 'A<CR>second<Esc>'},
    {'completed backslash', {'value = first + \\', '        second'}, 'A<CR>next_value = 2<Esc>'},
    {'backslash inside call', {'value = call(first + \\', '             second', ')'}, 'kA<CR>+ third<Esc>'},
    {'unfinished string', {'def f():', '    text = """', '    literal:'}, 'A<CR>literal<Esc>'},
    {'comment inside unfinished literal', {'def f():', '    text = """', '    # note'}, 'A<CR>literal<Esc>'},
    {'comment inside closed literal', {'text = """', '# note', '"""'}, 'kA<CR>literal<Esc>'},
    {'paste', {'def f():', '    pass'}, 'A<CR><Esc>', '    value = [\n        1,\n    ]'},
  }
  for _, case in ipairs(replays) do
    local native = replay('python', case[2], case[3], case[4])
    local notebook = replay('markdown', case[2], case[3], case[4])
    assert(vim.deep_equal(notebook, native), case[1] .. ' mapped Enter replay differs: native='
      .. vim.inspect(native) .. ' notebook=' .. vim.inspect(notebook))
  end
  local boundary = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(boundary, 0, -1, false, {'```python', 'value = 1', '```'})
  vim.bo[boundary].filetype = 'markdown'
  vim.b[boundary].notebook_markdown = true
  keeper.rafts[boundary] = {buffers={python=embedded}}
  vim.bo[boundary].shiftwidth, vim.bo[boundary].tabstop = 2, 2
  indent.attach(boundary)
  vim.api.nvim_set_current_buf(boundary)
  vim.api.nvim_win_set_cursor(0, {2, 0})
  vim.api.nvim_exec_autocmds('CursorMoved', {buffer=boundary})
  for _, fence in ipairs({'```python', '````python', '`````py'}) do
    local delimiter = fence:match('^`+')
    vim.api.nvim_buf_set_lines(boundary, 0, -1, false, {fence, 'value = 1', delimiter})
    vim.api.nvim_exec_autocmds('CursorMoved', {buffer=boundary})
    assert(vim.bo[boundary].shiftwidth == python_options.shiftwidth, 'Python fence variant lost indentation settings')
  end
  vim.api.nvim_buf_set_lines(boundary, 0, -1, false, {'```text', 'value = 1', '```'})
  vim.api.nvim_exec_autocmds('CursorMoved', {buffer=boundary})
  assert(vim.bo[boundary].shiftwidth == 2, 'editing the fence language did not restore Markdown options')
  vim.api.nvim_buf_set_lines(boundary, 0, -1, false, {'````python', '', '````'})
  vim.api.nvim_exec_autocmds('CursorMoved', {buffer=boundary})
  assert(vim.bo[boundary].shiftwidth == python_options.shiftwidth, 'empty cell did not inherit Python indentation')
  vim.api.nvim_feedkeys(vim.keycode('idef f():<CR>pass<Esc>'), 'mtx', false)
  assert(vim.api.nvim_buf_get_lines(boundary, 2, 3, false)[1] == '    pass', 'typing in an empty cell used Markdown indentation')
  keeper.rafts[boundary] = nil
  vim.api.nvim_buf_delete(boundary, {force=true})
  vim.api.nvim_set_current_buf(indent_buf)
  vim.api.nvim_win_set_cursor(0, {1, 0})
  vim.api.nvim_exec_autocmds('CursorMoved', {buffer=indent_buf})
  assert(vim.bo[indent_buf].comments == markdown_comments[1]
    and vim.bo[indent_buf].commentstring == markdown_comments[2]
    and vim.bo[indent_buf].formatoptions == markdown_comments[3],
    'leaving Python must restore Markdown comment and format settings')
  vim.api.nvim_win_set_cursor(0, {4, 0})
  vim.api.nvim_exec_autocmds('CursorMoved', {buffer=indent_buf})
  local code_win = vim.api.nvim_get_current_win()
  vim.cmd('vsplit')
  local prose_win = vim.api.nvim_get_current_win()
  vim.api.nvim_win_set_cursor(prose_win, {1, 0})
  vim.api.nvim_set_current_win(code_win)
  assert(vim.bo[indent_buf].shiftwidth == python_options.shiftwidth,
    'switching back to the Python window must restore Python indentation settings')
  for name, value in pairs(python_options) do
    assert(vim.bo[indent_buf][name] == value, 'switching windows lost Python option ' .. name)
  end
  assert(vim.deep_equal(vim.api.nvim_win_get_cursor(code_win), {4, 0}), 'switching options moved the Python cursor')
  vim.api.nvim_set_current_win(prose_win)
  assert(vim.bo[indent_buf].shiftwidth == 2,
    'switching to the prose window must restore Markdown indentation settings')
  assert(vim.deep_equal(vim.api.nvim_win_get_cursor(prose_win), {1, 0}), 'switching options moved the prose cursor')
  vim.cmd('close')
  vim.api.nvim_win_set_cursor(0, {1, 0})
  vim.api.nvim_exec_autocmds('CursorMoved', {buffer=indent_buf})
  assert(vim.bo[indent_buf].shiftwidth == 2 and vim.bo[indent_buf].tabstop == 2,
    'prose indentation settings were not restored outside a Python fence')
  vim.api.nvim_set_current_buf(buf)
  keeper.rafts[indent_buf] = nil
  vim.api.nvim_buf_delete(indent_buf, {force=true})
  assert((vim.g.ipynb_test_rpcs.code_spans or 0) == 0, 'opening repeated the preview span conversion')
  result.open_rpcs = vim.g.ipynb_test_rpcs
  local executable = require('ipynb.cells').executable(buf)
  assert(#executable == 2 and #require('ipynb.cells').spans(buf) == 3, 'preview spans include prose code fences')
  assert((vim.g.ipynb_test_rpcs.code_spans or 0) == 0, 'preview spans were not cached')
  local cell_ranges = require('ipynb.cells')
  local geometry_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_name(geometry_buf, vim.fn.tempname() .. '.ipynb')
  vim.api.nvim_buf_set_lines(geometry_buf, 0, -1, false, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
  vim.b[geometry_buf].ipynb_code_spans = executable
  vim.b[geometry_buf].ipynb_code_spans_tick = vim.api.nvim_buf_get_changedtick(geometry_buf)
  cell_ranges.executable(geometry_buf)
  local function body_edit(first, last, replacement)
    vim.api.nvim_buf_set_lines(geometry_buf, first, last, false, replacement)
    local count = vim.g.ipynb_test_rpcs.code_spans or 0
    local cached = cell_ranges.executable(geometry_buf)
    assert((vim.g.ipynb_test_rpcs.code_spans or 0) == count, 'body edits repeated notebook conversion')
    assert(vim.deep_equal(cached, cell_ranges.executable(geometry_buf, true)),
      'cached body geometry disagrees with Jupytext executable membership')
  end
  local body = executable[1].begin[1]
  body_edit(body, executable[1]['end'][1] + 1, {})
  body_edit(body, body, {'', 'print(42)'})
  body_edit(body, body + 1, {'value = 1'})
  body_edit(body + 1, body + 2, {'print(value)', 'print(42)'})
  body_edit(body, body + 2, {'value = 1; print(value)'})
  local following = cell_ranges.executable(geometry_buf)[2].begin[1]
  body_edit(following, following + 1, {''})
  body_edit(following, following + 1, {'print(43)'})
  vim.api.nvim_buf_delete(geometry_buf, {force=true})
  vim.wo.number, vim.wo.relativenumber = true, true
  local function gutter() return {vim.wo.number, vim.wo.relativenumber, vim.wo.statuscolumn} end
  local initial_gutter = gutter()
  local markdown = require('ipynb.markdown')
  local highlighter = assert(vim.treesitter.highlighter.active[buf])
  assert(vim.filetype.match({buf=buf}) == 'markdown', 'quickfile detection must respect converted notebook text')
  vim.treesitter.start(buf, vim.treesitter.language.get_lang(vim.filetype.match({buf=buf})))
  assert(vim.treesitter.highlighter.active[buf] == highlighter,
    'quickfile must not replace notebook highlighting with a JSON parser')
  assert(vim.filetype.match({filename='ordinary.py'}) == 'python')
  assert(vim.filetype.match({filename='unloaded.ipynb'}) == 'json')
  local highlight_query = highlighter:get_query('markdown'):query()
  local tree = assert(vim.treesitter.get_parser(buf, 'markdown'):parse()[1])
  for capture, _, metadata in highlight_query:iter_captures(tree:root(), buf) do
    assert(not metadata.conceal_lines and not (metadata[capture] or {}).conceal_lines,
      'Markdown highlights hide fence rows and their anchored output')
  end
  assert(vim.treesitter.query.get('markdown', 'highlights').has_conceal_line,
    'notebook highlighting must not change other Markdown buffers')
  local namespace = vim.api.nvim_get_namespaces()['ipynb-markdown']
  local function fences()
    local rows = {}
    for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buf, namespace, 0, -1, {details=true})) do
      if mark[4].conceal == '' then rows[#rows + 1] = mark[2] end
    end
    return rows
  end
  result.initial_fences = fences()
  local before = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  vim.cmd('syntax clear')
  markdown.enable(buf)
  result.recovered_fences = fences()
  local function geometry()
    markdown.refresh(buf)
    local marks = vim.api.nvim_buf_get_extmarks(buf, namespace, 0, -1, {details=true})
    local edges = {}
    for _, mark in ipairs(marks) do
      local text
      if mark[4].virt_text then
        local chunks = {}
        for _, chunk in ipairs(mark[4].virt_text) do chunks[#chunks + 1] = chunk[1] end
        text = table.concat(chunks)
      end
      if text and (text:find('╭', 1, true) or text:find('╰', 1, true)) then
        edges[#edges + 1] = {text=text, width=vim.fn.strdisplaywidth(text)}
      end
    end
    return { width=vim.fn.getwininfo(vim.api.nvim_get_current_win())[1].width - vim.fn.getwininfo(vim.api.nvim_get_current_win())[1].textoff,
      marks=marks, edges=edges }
  end
  result.wide = geometry()
  local metadata_end
  for row = 3, #before do if before[row] == '---' then metadata_end = row; break end end
  assert(vim.fn.foldclosedend(1) == metadata_end, 'metadata is hidden without a navigable fold')
  vim.cmd('normal! gg2j')
  assert(vim.api.nvim_win_get_cursor(0)[1] == metadata_end + 2, 'movement entered hidden metadata')
  vim.cmd('normal! 2k')
  assert(vim.api.nvim_win_get_cursor(0)[1] == 1, 'upward movement entered hidden metadata')
  vim.cmd('split')
  markdown.enable(buf)
  assert(vim.fn.foldclosedend(1) == metadata_end, 'split lost the metadata fold')
  vim.cmd('normal! ggj')
  assert(vim.api.nvim_win_get_cursor(0)[1] == metadata_end + 1,
    'split navigation entered metadata: ' .. vim.inspect(vim.api.nvim_win_get_cursor(0)))
  vim.cmd('close')
  local rails = {}
  for _, mark in ipairs(result.wide.marks) do
    local opts = mark[4]
    if opts.virt_text and (opts.virt_text_pos == 'inline' or opts.virt_text_pos == 'eol_right_align') then
      rails[mark[2]] = (rails[mark[2]] or 0) + 1
    end
  end
  for row, line in ipairs(before) do
    if line:match('^print%(') then assert(rails[row - 1] == 2, 'code body is missing its side borders') end
  end
  if require('ipynb.markdown_integration').active() then
    local ns = vim.api.nvim_get_namespaces()['render-markdown.nvim']
    local marks = vim.api.nvim_buf_get_extmarks(buf, ns, 0, -1, {details=true})
    assert(#marks > 0, 'normal Markdown renderer did not attach')
    local protected, fenced = {[0]=true}, false
    for _, mark in ipairs(result.wide.marks) do if mark[4].conceal_lines then protected[mark[2]] = true end end
    for row, line in ipairs(before) do
      if line:match('^```') then protected[row - 1] = true; fenced = not fenced
      elseif fenced then protected[row - 1] = true end
    end
    for _, mark in ipairs(marks) do
      assert(not protected[mark[2]],
        'prose renderer decorated notebook metadata or code: ' .. vim.inspect(mark))
    end
    for _, mark in ipairs(result.wide.marks) do
      assert(not (mark[4].line_hl_group or ''):find('Heading'), 'duplicate prose renderer')
    end
    local plain = vim.api.nvim_create_buf(true, false)
    vim.api.nvim_buf_set_name(plain, vim.fn.tempname() .. '.md')
    vim.api.nvim_set_current_buf(plain)
    vim.api.nvim_buf_set_lines(plain, 0, -1, false, {'# Ordinary Markdown', '', '```python', 'print(1)', '```'})
    vim.bo[plain].filetype = 'markdown'
    require('render-markdown').render({buf=plain})
    vim.wait(200)
    assert(#vim.api.nvim_buf_get_extmarks(plain, ns, 2, -1, {}) > 0, 'ordinary Markdown code lost its renderer')
    vim.api.nvim_set_current_buf(buf)
    vim.api.nvim_buf_delete(plain, {force=true})
  end
  for _, theme in ipairs({'habamax', 'quiet', 'default'}) do
    vim.o.background = theme == 'quiet' and 'light' or 'dark'
    vim.cmd.colorscheme(theme)
    local function hl(name) return vim.api.nvim_get_hl(0, {name=name, link=false}) end
    assert(hl('IpynbVirtualText').bg == hl('IpyNotebookCodeBlock').bg, 'code and output backgrounds differ')
    assert(hl('IpynbOutputBorderFail').fg == hl('DiagnosticError').fg, 'error color lost theme link')
    assert(hl('IpyNotebookCodeBlock').fg == nil, 'cell fill hides syntax colors')
  end
  vim.api.nvim_set_hl(0, 'Normal', {fg=0xeeeeee})
  vim.api.nvim_set_hl(0, 'NormalFloat', {})
  vim.api.nvim_set_hl(0, 'RenderMarkdownCode', {bg=0x181825})
  require('ipynb.highlights').setup()
  assert(vim.api.nvim_get_hl(0, {name='IpyNotebookCodeBlock', link=false}).bg == 0x181825, 'code ignores the theme code background')
  assert(vim.api.nvim_get_hl(0, {name='IpynbVirtualText', link=false}).bg == 0x181825, 'output ignores the theme code background')
  vim.api.nvim_set_hl(0, 'RenderMarkdownCode', {})
  require('ipynb.highlights').setup()
  assert(vim.api.nvim_get_hl(0, {name='IpyNotebookCodeBlock', link=false}).bg == nil, 'opaque fallback on a transparent theme')
  vim.o.columns = 60
  result.narrow = geometry()
  markdown.disable(buf)
  result.raw_metadata = #vim.api.nvim_buf_get_extmarks(buf, namespace, 0, -1, {}) == 0
  markdown.enable(buf)
  result.source_unchanged = vim.deep_equal(before, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
  local insert_visible, late_completion
  local insert_span = require('ipynb.cells').executable(buf)[1]
  vim.api.nvim_win_set_cursor(0, {insert_span.begin[1] + 1, 0})
  local insert_conversions = vim.g.ipynb_test_rpcs.code_spans or 0
  vim.defer_fn(function()
    for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buf, namespace, 0, -1, {details=true})) do
      assert(mark[2] ~= insert_span.begin[1] - 1 or mark[4].virt_text_pos ~= 'overlay',
        'entering Insert without typing must reveal the opening fence')
    end
    markdown.enable(buf)
    insert_visible = vim.api.nvim_get_mode().mode == 'i'
      and vim.fn.foldclosedend(1) == metadata_end
      and #vim.api.nvim_buf_get_extmarks(buf, namespace, 0, -1, {}) > 0
      and #vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces()['ipynb-cell-header'], 0, -1, {}) > 0
    vim.api.nvim_buf_set_lines(buf, insert_span['end'][1] + 1, insert_span['end'][1] + 1, false, {'# inserted while editing'})
    local cells = require('ipynb.cells')
    local parse, parses = cells.spans, 0
    cells.spans = function(...) parses = parses + 1; return parse(...) end
    vim.api.nvim_exec_autocmds('TextChangedI', {buffer=buf})
    for _ = 1, 3 do require('ipynb.native_ui').refresh(buf) end
    cells.spans = parse
    insert_visible = insert_visible and parses <= 1
    local expanded = false
    for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buf, namespace, 0, -1, {details=true})) do
      if mark[2] == insert_span['end'][1] + 2 and mark[4].virt_text_pos == 'overlay' then expanded = true end
    end
    insert_visible = insert_visible and not expanded and (vim.g.ipynb_test_rpcs.code_spans or 0) == insert_conversions
    result.insert_debug = {visible=insert_visible, expanded=expanded, parses=parses,
      conversions=vim.g.ipynb_test_rpcs.code_spans, original_conversions=insert_conversions,
      active=vim.b[buf].ipynb_markdown_active_cell, mode=vim.api.nvim_get_mode().mode,
      fold=vim.fn.foldclosedend(1), metadata_end=metadata_end,
      headers=#vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces()['ipynb-cell-header'], 0, -1, {})}
    if package.loaded['blink.cmp'] then
      local menu = require('blink.cmp.completion.windows.menu')
      local context = require('blink.cmp.completion.trigger.context').new({id=99999, providers={},
        initial_trigger_kind='manual', trigger_kind='manual'})
      local wrap = vim.schedule_wrap
      vim.schedule_wrap = function(callback) late_completion = callback; return wrap(callback) end
      local opened, failure = pcall(menu.open_loading, context)
      vim.schedule_wrap = wrap
      assert(opened, failure)
      menu.close()
    end
    vim.api.nvim_input(vim.keycode('<Esc>'))
  end, 200)
  vim.api.nvim_feedkeys('i', 'nx!', false)
  vim.wait(200)
  assert(insert_visible, 'Insert mode must reveal the active cell without converter calls')
  assert(vim.fn.foldclosedend(1) == metadata_end, 'metadata fold did not recover after Insert')
  if late_completion then
    late_completion()
    assert(not require('blink.cmp.completion.windows.menu').win:is_open(), 'delayed completion reopened in Normal mode')
  end
  assert(#vim.api.nvim_buf_get_extmarks(buf, namespace, 0, -1, {}) > 0, 'rendering did not recover after Insert')
  require('ipynb').options.auto_setup = false
  local spans = {}
  for _, candidate in ipairs(require('ipynb.cells').spans(buf)) do
    if (vim.api.nvim_buf_get_lines(buf, candidate.begin[1], candidate.begin[1] + 1, false)[1] or ''):match('^print%(') then
      spans[#spans + 1] = candidate
    end
  end
  local span = spans[1]
  vim.api.nvim_win_set_cursor(0, {span.begin[1], 0})
  vim.cmd([[normal \r]])
  vim.wait(10000, function()
    for _, cell in ipairs(require('ipynb.native_ui').cells(buf)) do
      if cell.status == 'done' and not cell.old then return true end
    end
  end, 20)
  result.cells = require('ipynb.native_ui').cells(buf)
  result.output = vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces()['ipynb-native-output'], 0, -1, {details=true})
  vim.cmd('IpynbEnterOutput')
  result.output_opened = vim.api.nvim_get_current_buf() ~= buf
  if result.output_opened then
    vim.fn.maparg('q', 'n', false, true).callback()
    assert(vim.api.nvim_get_current_buf() == buf, 'q must return to the notebook')
    vim.cmd([[normal \o]])
    assert(vim.api.nvim_get_current_buf() == buf, 'the source output key must collapse after q, not reopen a float')
    vim.cmd([[normal \o]])
    assert(vim.api.nvim_get_current_buf() ~= buf, 'the source output key must expand the collapsed preview')
    vim.cmd('IpynbHideOutput')
  end
  vim.api.nvim_win_set_cursor(0, {span['end'][1] + 2, 0})
  vim.cmd('IpynbEnterOutput')
  result.bottom_opened = vim.api.nvim_get_current_buf() ~= buf
  if result.bottom_opened then vim.cmd('IpynbHideOutput') end
  local function run(keys, row, index)
    index = index or 1
    vim.api.nvim_win_set_cursor(0, {row, 0})
    local count = tonumber(require('ipynb.native_ui').cells(buf)[index].execution_count) or 0
    vim.cmd.normal(keys)
    vim.wait(10000, function()
      local cell = require('ipynb.native_ui').cells(buf)[index]
      return cell.status == 'done' and (tonumber(cell.execution_count) or 0) > count
    end, 20)
    return (tonumber(require('ipynb.native_ui').cells(buf)[index].execution_count) or 0) > count
  end
  result.above = run([[\a]], spans[2].begin[1], 2)
  result.all = run([[\R]], 1, 2)
  result.executed_cells = #require('ipynb.native_ui').cells(buf)
  result.after_all = require('ipynb.native_ui').cells(buf)
  result.visual = run([[V\r]], span.begin[1] + 1)
  vim.api.nvim_win_set_cursor(0, {span.begin[1], 0})
  vim.fn.maparg('<S-CR>', 'n', false, true).callback()
  result.advanced = vim.api.nvim_win_get_cursor(0)[1] > span['end'][1] + 1
  local ui = require('ipynb.native_ui')
  local cells = ui.cells(buf)
  local output_ns = vim.api.nvim_get_namespaces()['ipynb-native-output']
  vim.api.nvim_win_set_cursor(0, {span.begin[1] + 1, 0})
  ui.refresh(buf)
  local header_ns = vim.api.nvim_get_namespaces()['ipynb-cell-header']
  local stationary_headers = vim.api.nvim_buf_get_extmarks(buf, header_ns, 0, -1, {details=true})
  for _ = 1, 10 do
    require('ipynb.native').call('IpynbOnCursorMoved')
    require('ipynb.native').call('IpynbOnWinScrolled')
  end
  assert(vim.deep_equal(stationary_headers, vim.api.nvim_buf_get_extmarks(buf, header_ns, 0, -1, {details=true})),
    'cursor movement and scrolling must not replace unchanged cell decorations')
  for _, state in ipairs({
    {'hold', true, false, 'Queued', 'Queued'}, {'running', true, false, 'Running', 'Running'},
    {'done', true, false, 'Done', 'Success'}, {'done', false, false, 'Failed', 'Fail'},
    {'done', true, true, 'Previous', 'Stale'},
  }) do
    local snapshot = vim.deepcopy(cells)
    snapshot[1].status, snapshot[1].success, snapshot[1].old = state[1], state[2], state[3]
    ui.sync(buf, snapshot)
    local marks = vim.api.nvim_buf_get_extmarks(buf, output_ns, 0, -1, {details=true})
    assert(marks[1][4].virt_lines[1][1][2] == 'Normal', 'output must have a normal-background gap')
    local header = marks[1][4].virt_lines[2][1]
    assert(header[1]:find(state[4], 1, true) and header[2] == 'IpynbOutputBorder' .. state[5], 'wrong inline state')
    vim.cmd('IpynbEnterOutput')
    assert(vim.api.nvim_get_current_buf() ~= buf, 'state output did not open')
    assert(vim.api.nvim_buf_get_lines(0, 0, 1, false)[1]:find(state[4], 1, true), 'wrong floating state')
    local floating = vim.api.nvim_buf_get_extmarks(0, output_ns, 0, -1, {details=true})
    assert(floating[1][4].hl_group == header[2], 'floating header lost status styling')
    ui.hide(false)
  end
  ui.sync(buf, cells)
  assert(#vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces()['ipynb-native-highlight'], 0, -1, {}) == 0,
    'entering a code cell added a different focus background')
  assert(vim.deep_equal(gutter(), initial_gutter), 'notebook UI changed the line-number gutter')
  vim.wo.numberwidth = 8
  vim.wait(100)
  local info = vim.fn.getwininfo(vim.api.nvim_get_current_win())[1]
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(buf, output_ns, 0, -1, {details=true})) do
    local line = ''
    for _, chunk in ipairs(mark[4].virt_lines[1]) do line = line .. chunk[1] end
    assert(vim.fn.strdisplaywidth(line) == info.width - info.textoff, 'output did not follow the new gutter width')
  end
  local corners = vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces()['ipynb-cell-header'], 0, -1, {details=true})
  assert(#corners == 2 and corners[1][4].virt_text[1][1]:find('Cell 1', 1, true), 'cell identity is missing or prose was numbered')
  local conversions = vim.g.ipynb_test_rpcs.code_spans or 0
  require('ipynb.cells').add('code')
  corners = vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces()['ipynb-cell-header'], 0, -1, {details=true})
  assert(#corners == 3 and corners[3][4].virt_text[1][1]:find('3', 1, true), 'new cells were not numbered before execution')
  assert((vim.g.ipynb_test_rpcs.code_spans or 0) > conversions, 'editing did not invalidate preview spans')
  conversions = vim.g.ipynb_test_rpcs.code_spans or 0
  vim.cmd('edit!')
  vim.wait(100)
  assert((vim.g.ipynb_test_rpcs.code_spans or 0) == conversions, 'reopening repeated the preview span conversion')
  assert(#require('ipynb.cells').executable(buf) == 2, 'reopening reused edited spans')
  local reloaded = require('ipynb.native_ui').cells(buf)
  assert(#reloaded == 2 and reloaded[1].source:match('^print%(42%)') and reloaded[2].source:match('^print%(43%)'),
    'reopening reused collapsed cell positions: ' .. vim.inspect(reloaded))
  local previews, preview_lines = {}, {'print(7)', ''}
  for index = 1, 2 do
    local preview = vim.api.nvim_create_buf(false, true)
    previews[index] = preview
    vim.cmd('vsplit')
    vim.api.nvim_set_current_buf(preview)
    vim.api.nvim_buf_set_lines(preview, 0, -1, false, preview_lines)
    local saved = vim.tbl_extend('force', reloaded[1], {
      id = 'saved-preview-' .. index, begin = {0, 0}, ['end'] = {0, 8}, old = true, lines = {'7'},
    })
    ui.sync(preview, {saved})
    assert(#require('ipynb.native').running(preview) == 0, 'saved preview started a kernel')
  end
  local other_preview = ui.cells(previews[2])
  assert(ui.show('saved-preview-1'))
  local preview_float
  for _, win in ipairs(vim.api.nvim_list_wins()) do
    if vim.api.nvim_win_get_config(win).relative ~= '' then preview_float = win end
  end
  assert(preview_float and vim.api.nvim_get_current_buf() == previews[2], 'preview output stole notebook focus')
  local wiped_lines
  vim.api.nvim_create_autocmd('BufWipeout', {buffer=previews[1], once=true, callback=function()
    wiped_lines = vim.api.nvim_buf_get_lines(previews[1], 0, -1, false)
  end})
  vim.api.nvim_buf_delete(previews[1], {force=true})
  assert(#ui.cells(previews[1]) == 0 and not vim.api.nvim_win_is_valid(preview_float), 'unload deferred UI cleanup')
  require('ipynb.native').request('tick') -- Drain queued cleanup before checking ownership.
  assert(vim.deep_equal(wiped_lines, preview_lines)
    and vim.deep_equal(vim.api.nvim_buf_get_lines(previews[2], 0, -1, false), preview_lines)
    and vim.deep_equal(vim.api.nvim_buf_get_lines(buf, 0, -1, false), before), 'preview cleanup changed source')
  assert(vim.deep_equal(ui.cells(previews[2]), other_preview), 'wiping one notebook changed another preview')
  assert(#ui.cells(previews[1]) == 0, 'wiped kernel-free notebook retained saved preview UI')
  assert(not vim.api.nvim_win_is_valid(preview_float), 'wiped notebook retained its output window')
  local directory = vim.fn.fnamemodify(vim.env.IPYNB_VIEW_REPORT, ':h')
  local relative = directory .. '/relative.ipynb'
  vim.fn.writefile(vim.fn.readfile(directory .. '/view.ipynb', 'b'), relative, 'b')
  vim.cmd('cd ' .. vim.fn.fnameescape(directory))
  vim.cmd('edit relative.ipynb')
  local saved_buf = vim.api.nvim_get_current_buf()
  local keeper = require('otter.keeper')
  local raft = assert(keeper.rafts[saved_buf])
  local embedded = raft.buffers.python
  local namespace = assert(raft.diagnostics_namespaces[embedded])
  local source_a = vim.api.nvim_create_namespace('ipynb-test-diagnostics-a')
  local source_b = vim.api.nvim_create_namespace('ipynb-test-diagnostics-b')
  local function diagnostic(message, col)
    return {lnum=0, col=col or 0, end_lnum=0, end_col=(col or 0) + 1,
      severity=vim.diagnostic.severity.ERROR, message=message}
  end
  local first = diagnostic('provider a')
  first.end_lnum = 1
  vim.diagnostic.set(source_a, embedded, {first})
  vim.diagnostic.set(source_b, embedded, {diagnostic('provider b')})
  local function relay()
    vim.api.nvim_exec_autocmds('DiagnosticChanged', {buffer=embedded,
      data={diagnostics={diagnostic('latest provider event')}}})
  end
  relay()
  local function parent_messages()
    local messages = {}
    for _, item in ipairs(vim.diagnostic.get(saved_buf, {namespace=namespace})) do
      messages[item.message] = item
    end
    return messages
  end
  assert(parent_messages()['provider a'] and parent_messages()['provider b'],
    'Otter relay must preserve diagnostics from every embedded provider')
  local original_offset = keeper.get_leading_offset
  keeper.get_leading_offset = function(line) return line == 0 and 2 or 4 end
  relay()
  local first_relay = parent_messages()
  relay()
  keeper.get_leading_offset = original_offset
  assert(first_relay['provider a'].col == 2 and first_relay['provider a'].end_col == 5
    and parent_messages()['provider a'].col == 2 and parent_messages()['provider a'].end_col == 5
    and vim.diagnostic.get(embedded, {namespace=source_a})[1].col == 0,
    'diagnostic leading offsets must be restored without accumulating')
  vim.diagnostic.reset(source_b, embedded)
  relay()
  assert(parent_messages()['provider a'] and parent_messages()['provider b'] == nil,
    'clearing one embedded provider must preserve the other provider diagnostics')
  vim.diagnostic.reset(source_a, embedded)
  vim.diagnostic.reset(source_b, embedded)
  relay()
  assert(next(parent_messages()) == nil, 'synthetic provider diagnostics must be cleared after the relay check')
  local refresh, refresh_calls = vim.lsp.diagnostic._refresh, {}
  vim.lsp.diagnostic._refresh = function(buf, client, only_visible)
    refresh_calls[#refresh_calls + 1] = {buf=buf, client=client, only_visible=only_visible}
  end
  local function notify(buf, method, client)
    vim.api.nvim_exec_autocmds('LspNotify', {buffer=buf,
      data={method=method, client_id=client}})
  end
  refresh_calls = {}
  notify(embedded, 'textDocument/didChange', 123)
  notify(embedded, 'textDocument/didSave', 124)
  notify(saved_buf, 'textDocument/didChange', 125)
  local hidden_refreshes = {}
  for _, call in ipairs(refresh_calls) do
    if call.only_visible ~= true then hidden_refreshes[#hidden_refreshes + 1] = call end
  end
  assert(#hidden_refreshes == 1 and hidden_refreshes[1].buf == embedded and hidden_refreshes[1].client == 123,
    'hidden embedded didChange must refresh only its notifying LSP client: ' .. vim.inspect(refresh_calls))
  local list_wins, win_buf = vim.api.nvim_list_wins, vim.api.nvim_win_get_buf
  vim.api.nvim_list_wins = function() return {vim.api.nvim_get_current_win()} end
  vim.api.nvim_win_get_buf = function() return embedded end
  notify(embedded, 'textDocument/didChange', 126)
  vim.api.nvim_list_wins, vim.api.nvim_win_get_buf = list_wins, win_buf
  local visible_refreshes = 0
  for _, call in ipairs(refresh_calls) do if call.only_visible ~= true then visible_refreshes = visible_refreshes + 1 end end
  assert(visible_refreshes == 1, 'visible embedded buffers must keep Neovim diagnostic refresh behavior: '
    .. vim.inspect(refresh_calls))
  vim.lsp.diagnostic._refresh = refresh
  local sync_raft, sync_calls = keeper.sync_raft, 0
  keeper.sync_raft = function(buf) assert(buf == saved_buf); sync_calls = sync_calls + 1 end
  for _, event in ipairs({'TextChanged', 'TextChangedI', 'TextChangedP'}) do
    vim.api.nvim_exec_autocmds(event, {buffer=saved_buf})
  end
  assert(vim.wait(500, function() return sync_calls > 0 end, 5), 'editing must schedule an Otter sync')
  vim.wait(50)
  keeper.sync_raft = sync_raft
  assert(sync_calls == 1, 'multiple edit events must coalesce into one scheduled Otter sync')
  assert(vim.bo[saved_buf].buftype == '' and vim.b[saved_buf].autoformat == nil,
    'notebooks must inherit normal save and autoformat settings')
  local format_calls = 0
  local format_buffers, python_filetypes = {}, 0
  local save_windows = #vim.api.nvim_list_wins()
  local format_group = vim.api.nvim_create_augroup('NotebookConfiguredFormatter', {})
  vim.api.nvim_create_autocmd('FileType', {group=format_group, pattern='python', callback=function()
    python_filetypes = python_filetypes + 1
  end})
  vim.api.nvim_create_autocmd('BufWritePre', {group=format_group, pattern='*.ipynb', callback=function(event)
    local text = vim.api.nvim_buf_get_lines(event.buf, 0, -1, false)
    vim.api.nvim_buf_set_lines(event.buf, 0, -1, false, text)
  end})
  vim.api.nvim_create_autocmd('BufWritePre', {group=format_group, pattern='*.py', callback=function(event)
    assert(vim.bo[event.buf].filetype == 'python', 'code formatter received the wrong filetype')
    local win = vim.api.nvim_get_current_win()
    assert(vim.api.nvim_win_get_config(win).hide == true and vim.api.nvim_win_get_config(win).focusable == false
      and vim.w[win].ipynb_editor == true,
      'formatters must not display temporary buffers over the notebook during a save')
    format_calls = format_calls + 1
    format_buffers[format_calls] = event.buf
    local text = vim.api.nvim_buf_get_lines(event.buf, 0, -1, false)
    for index, line in ipairs(text) do text[index] = line:gsub('print%(  44  %)', 'print(44)') end
    vim.api.nvim_buf_set_lines(event.buf, 0, -1, false, text)
  end})
  local span = require('ipynb.cells').executable(saved_buf)[1]
  local code_spans = require('ipynb.cells').spans(saved_buf)
  local longer = code_spans[3]
  for _, row in ipairs({longer.begin[1] - 1, longer['end'][1] + 1}) do
    vim.api.nvim_buf_set_text(saved_buf, row, 0, row, 0, {'`'})
  end
  vim.api.nvim_buf_set_lines(saved_buf, longer.begin[1], longer['end'][1] + 1, false, {'print(  44  )'})
  vim.api.nvim_buf_set_lines(saved_buf, span['end'][1] + 1, span['end'][1] + 1, false, {'print(  44  )'})
  vim.api.nvim_exec_autocmds('TextChanged', {buffer=saved_buf})
  local position = ui.positions(saved_buf)[1]
  assert(position.source:find('print(  44  )', 1, true), 'paste was excluded from the cell source')
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(saved_buf, output_ns, 0, -1, {details=true})) do
    assert(mark[2] > position['end'][1], 'paste left output inside the code fence')
  end
  local notebook_win = vim.api.nvim_get_current_win()
  local notebook_cursor, notebook_view = vim.api.nvim_win_get_cursor(notebook_win), vim.api.nvim_win_call(notebook_win, vim.fn.winsaveview)
  vim.cmd('write')
  vim.cmd('write')
  assert(format_calls == 4, 'each save must use configured Python save hooks for both code cells')
  assert(python_filetypes == 2, 'each save must set Python filetype once for its shared formatter buffer')
  assert(format_buffers[1] == format_buffers[2] and format_buffers[3] == format_buffers[4]
    and format_buffers[1] ~= format_buffers[3], 'each save must reuse one fresh formatter buffer across its cells')
  assert(vim.api.nvim_get_current_win() == notebook_win
    and vim.deep_equal(vim.api.nvim_win_get_cursor(notebook_win), notebook_cursor)
    and vim.deep_equal(vim.api.nvim_win_call(notebook_win, vim.fn.winsaveview), notebook_view),
    'saving must preserve the notebook window focus, cursor, and view')
  assert(#vim.api.nvim_list_wins() == save_windows, 'saving must close its hidden formatter windows')
  local stateful = vim.api.nvim_create_augroup('NotebookStatefulFormatter', {})
  local once_calls = 0
  vim.api.nvim_create_autocmd('FileType', {group=stateful, pattern='python', callback=function(event)
    vim.api.nvim_create_autocmd('BufWritePre', {group=stateful, buffer=event.buf, once=true, callback=function()
      once_calls = once_calls + 1
    end})
  end})
  vim.cmd('write')
  assert(once_calls == 2, 'one-shot buffer-local save hooks must run for every cell')
  vim.api.nvim_clear_autocmds({group=stateful})
  local variable_calls = 0
  vim.api.nvim_create_autocmd('BufWritePre', {group=stateful, pattern='*.py', callback=function(event)
    if vim.b[event.buf].did_format then return end
    vim.b[event.buf].did_format = true
    variable_calls = variable_calls + 1
  end})
  vim.cmd('write')
  assert(variable_calls == 2, 'buffer-local formatter state must not skip another cell')
  vim.api.nvim_del_augroup_by_id(stateful)
  local focus
  for _, spec in ipairs(dofile(require('lazy.core.config').plugins['ipynb.nvim'].dir .. '/lazy.lua')) do
    if spec[1] == 'nvim-lualine/lualine.nvim' then focus = spec.opts end
  end
  assert(focus, 'statusline integration missing')
  local opts = {options={ignore_focus={'markdown'}}}
  focus(nil, opts)
  assert(opts.options.ignore_focus(notebook_win), 'statusline integration lost ignored filetypes')
  opts = {options={ignore_focus=function(win) return win == notebook_win end}}
  focus(nil, opts)
  assert(opts.options.ignore_focus(notebook_win), 'statusline integration lost the configured callback')
  opts = {}
  focus(nil, opts)
  assert(not opts.options.ignore_focus(notebook_win), 'normal windows must retain statusline focus')
  vim.w[notebook_win].ipynb_editor = true
  assert(opts.options.ignore_focus(notebook_win), 'hidden notebook tools must not take statusline focus')
  vim.w[notebook_win].ipynb_editor = nil
  if vim.fn.exists('#general_autosave#FocusLost') == 1 then
    local native = require('ipynb.native')
    local request, nested_ok, nested_error = native.request
    local queued, writes = false, 0
    native.request = function(method, params)
      if method == 'write' then writes = writes + 1 end
      if method == 'write' and not queued then
        queued = true
        vim.defer_fn(function()
          vim.api.nvim_buf_set_lines(saved_buf, span.begin[1] + 1, span.begin[1] + 1, false, {'# typed during save'})
          nested_ok, nested_error = pcall(vim.api.nvim_exec_autocmds, 'FocusLost', {modeline=false})
        end, 0)
      end
      return request(method, params)
    end
    vim.api.nvim_buf_set_lines(saved_buf, span.begin[1] + 1, span.begin[1] + 1, false, {'# autosaved edit'})
    vim.api.nvim_exec_autocmds('FocusLost', {modeline=false})
    assert(vim.wait(2000, function() return not vim.bo[saved_buf].modified end, 5), 'queued edit was not saved')
    native.request = request
    assert(writes == 2, 'overlapping save must coalesce into one followup for the new edit')
    assert(not vim.fn.execute('messages'):find('notebook changed on disk since it was opened', 1, true))
    assert(queued and nested_ok, 'overlapping FocusLost save failed: ' .. tostring(nested_error))
    assert(not vim.bo[saved_buf].modified, 'FocusLost autosave must complete through configured save hooks')
    assert(vim.deep_equal(vim.b[saved_buf].mtime, vim.uv.fs_stat(relative).mtime), 'autosave left a stale disk version')
  end
  assert(type(vim.b[saved_buf].mtime) == 'table' and not vim.bo[saved_buf].modified,
    'repeated relative-path saves must retain the disk modification guard')
  local saved = vim.json.decode(table.concat(vim.fn.readfile(relative), '\n'))
  assert(table.concat(saved.cells[2].source):find('print(44)', 1, true), 'saved notebook lost pasted source')
  assert(table.concat(saved.cells[3].source):find('print(44)', 1, true),
    'configured formatter must process a code cell with a four-backtick fence')
  if vim.fn.exists('#general_autosave#FocusLost') == 1 then
    assert(table.concat(saved.cells[2].source):find('# typed during save', 1, true), 'save lost an in-flight edit')
    assert(table.concat(saved.cells[2].source):find('# autosaved edit', 1, true), 'autosave lost its original edit')
  end
  vim.opt.rtp:append(vim.env.IPYNB_TEST_PLUGINS .. '/nvim-lint')
  local lint = require('lint')
  local configured = lint.linters_by_ft.python
  lint.linters_by_ft.python = {'notebook_fixture'}
  lint.linters.notebook_fixture = {
    cmd='cat', stdin=true, args={}, stream='stdout',
    condition=function()
      assert(vim.api.nvim_win_get_config(0).hide == true,
        'linters must not display embedded buffers over the notebook during a save')
      return true
    end,
    parser=function(output)
      assert(output:find('print(44)', 1, true), 'configured linter did not receive notebook code')
      return {{lnum=span.begin[1],col=0,severity=vim.diagnostic.severity.WARN,message='configured notebook lint'}}
    end,
  }
  vim.cmd('write')
  assert(vim.wait(2000, function()
    for _, diagnostic in ipairs(vim.diagnostic.get(saved_buf)) do
      if diagnostic.message == 'configured notebook lint' then return true end
    end
  end, 10), 'configured Python lint diagnostics must appear on notebook source')
  local recovery_tick = vim.api.nvim_buf_get_changedtick(saved_buf)
  local old_embedded = require('otter.keeper').rafts[saved_buf].buffers.python
  require('otter').deactivate()
  assert(vim.wait(1000, function()
    return require('otter.keeper').rafts[saved_buf] == nil and not vim.api.nvim_buf_is_valid(old_embedded)
  end, 5))
  local scratch = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_set_current_buf(scratch)
  vim.api.nvim_set_current_buf(saved_buf)
  local recovered = require('otter.keeper').rafts[saved_buf]
  assert(recovered and vim.api.nvim_buf_is_loaded(recovered.buffers.python),
    'returning to an unchanged notebook must restore deactivated language tools')
  local recovered_sync, recovered_sync_calls = keeper.sync_raft, 0
  keeper.sync_raft = function(buf) assert(buf == saved_buf); recovered_sync_calls = recovered_sync_calls + 1 end
  vim.api.nvim_exec_autocmds('TextChanged', {buffer=saved_buf})
  assert(vim.wait(500, function() return recovered_sync_calls > 0 end, 5),
    'returning to an unchanged notebook must restore edit-triggered Otter sync')
  keeper.sync_raft = recovered_sync
  vim.treesitter.stop(saved_buf)
  vim.api.nvim_set_current_buf(scratch)
  vim.api.nvim_set_current_buf(saved_buf)
  assert(vim.treesitter.highlighter.active[saved_buf].ipynb_fences,
    'returning to an unchanged notebook must restore its custom highlighter')
  assert(vim.api.nvim_buf_get_changedtick(saved_buf) == recovery_tick, 'tool recovery must not edit the notebook')
  vim.api.nvim_buf_delete(scratch, {force=true})
  local disk_lint, write_to_disk = false, OtterConfig.buffers.write_to_disk
  local embedded = require('otter.keeper').rafts[saved_buf].buffers.python
  local embedded_path = vim.api.nvim_buf_get_name(embedded)
  local no_write = vim.api.nvim_get_autocmds({event='BufWriteCmd',buffer=embedded})
  for _, command in ipairs(no_write) do vim.api.nvim_del_autocmd(command.id) end
  OtterConfig.buffers.write_to_disk = true
  lint.linters.notebook_fixture.stdin = false
  lint.linters.notebook_fixture.parser = function(output)
    disk_lint = output:find('print(44)', 1, true) ~= nil
    return {}
  end
  vim.cmd('write')
  assert(vim.wait(2000, function() return disk_lint end, 10),
    'configured disk linter must receive current code when Otter disk buffers are enabled')
  OtterConfig.buffers.write_to_disk = write_to_disk
  for _, command in ipairs(no_write) do
    vim.api.nvim_create_autocmd('BufWriteCmd', {buffer=embedded,group=command.group,callback=command.callback})
  end
  vim.fn.delete(embedded_path)
  lint.linters_by_ft.python = configured
  local unchanged = table.concat(vim.fn.readfile(relative), '\n')
  vim.api.nvim_create_autocmd('BufWritePre', {group=format_group, pattern='*.py', callback=function()
    error('configured formatter failure')
  end})
  assert(not pcall(vim.cmd, 'write'), 'formatter failure must abort the notebook write')
  assert(#vim.api.nvim_list_wins() == save_windows, 'formatter failure must close its hidden window')
  assert(table.concat(vim.fn.readfile(relative), '\n') == unchanged, 'failed formatting changed the notebook file')
  vim.api.nvim_del_augroup_by_id(format_group)
  vim.cmd('write') -- A failed formatter must release the save guard.
  assert(vim.deep_equal(vim.b[saved_buf].mtime, vim.uv.fs_stat(relative).mtime))
  local outside = vim.fn.readfile(relative)
  outside[#outside + 1] = ' '
  vim.fn.writefile(outside, relative)
  local external = table.concat(vim.fn.readfile(relative), '\n')
  vim.api.nvim_buf_set_lines(saved_buf, span.begin[1] + 1, span.begin[1] + 1, false, {'# unsaved local edit'})
  assert(not pcall(vim.cmd, 'write'), 'external changes must still block save')
  assert(vim.bo[saved_buf].modified and table.concat(vim.fn.readfile(relative), '\n') == external,
    'a real conflict must preserve disk and unsaved edits')
  local recovery = relative .. '.recovery.ipynb'
  vim.cmd('write ' .. vim.fn.fnameescape(recovery))
  local recovered_copy = vim.json.decode(table.concat(vim.fn.readfile(recovery), '\n'))
  assert(table.concat(recovered_copy.cells[2].source):find('# unsaved local edit', 1, true), 'recovery copy lost local edits')
  assert(table.concat(vim.fn.readfile(relative), '\n') == external, 'recovery copy overwrote the conflicted notebook')
  vim.cmd('edit!')
  vim.cmd('write')
  assert(vim.deep_equal(vim.b[saved_buf].mtime, vim.uv.fs_stat(relative).mtime), 'reload did not recover saving')
  local loaded_disk = table.concat(vim.fn.readfile(relative), '\n')
  local read_request = require('ipynb.native').request
  require('ipynb.native').request = function(method, ...)
    if method == 'read' then error('injected notebook read failure') end
    return read_request(method, ...)
  end
  local read_ok = pcall(vim.cmd, 'edit!')
  require('ipynb.native').request = read_request
  assert(not read_ok, 'reload failure was not injected')
  local write_ok, read_error = pcall(vim.cmd, 'write')
  assert(not write_ok and tostring(read_error):find('did not finish loading', 1, true),
    'a failed reload must block saving the incomplete buffer')
  for _, operation in ipairs({{method='jupytext',action='sync',source='buffer'},{method='export'}}) do
    local params = vim.tbl_extend('force', {buf=saved_buf,path=relative,text='',overwrite=true}, operation)
    local request_ok, request_error = pcall(read_request, operation.method, params)
    assert(not request_ok and tostring(request_error):find('did not finish loading', 1, true),
      'failed reload must also block alternate notebook writers')
  end
  assert(table.concat(vim.fn.readfile(relative), '\n') == loaded_disk, 'failed reload allowed a destructive save')
  vim.cmd('edit!')
  vim.cmd('write')
  assert(vim.deep_equal(vim.b[saved_buf].mtime, vim.uv.fs_stat(relative).mtime), 'successful reload must restore saving')
  local jupytext = require('jupytext')
  local format, command = jupytext.opts.format, jupytext.opts.jupytext
  jupytext.opts.format, jupytext.opts.jupytext = 'py:percent', '/bin/false'
  local fallback_ok = pcall(jupytext.open_notebook, relative, saved_buf)
  jupytext.opts.jupytext = command
  assert(not fallback_ok and vim.b[saved_buf].ipynb_read_incomplete, 'legacy converter failure must keep saving blocked')
  assert(not pcall(vim.cmd, 'write'), 'legacy conversion failure allowed saving')
  assert(table.concat(vim.fn.readfile(relative), '\n') == loaded_disk, 'legacy converter failure changed notebook bytes')
  assert(jupytext.open_notebook(relative, saved_buf), 'legacy reader must recover after converter failure')
  assert(table.concat(vim.api.nvim_buf_get_lines(saved_buf, 0, -1, false), '\n'):find('print(44)', 1, true),
    'legacy conversion lost source')
  assert(not vim.b[saved_buf].ipynb_read_incomplete)
  jupytext.opts.format = format
  vim.cmd('edit!')
  local before_empty = vim.json.decode(table.concat(vim.fn.readfile(relative), '\n'))
  local deleting = require('ipynb.cells').executable(saved_buf)[1]
  assert(deleting['end'][1] > deleting.begin[1], 'line deletion fixture must contain multiple source rows')
  vim.api.nvim_buf_set_lines(saved_buf, deleting.begin[1], deleting.begin[1] + 1, false, {})
  vim.cmd('write')
  vim.cmd('edit!')
  vim.cmd('write')
  local after_delete = vim.json.decode(table.concat(vim.fn.readfile(relative), '\n'))
  assert(#after_delete.cells == #before_empty.cells, 'deleting a source line changed cell count')
  for index, cell in ipairs(before_empty.cells) do
    for _, key in ipairs({'id','metadata','outputs','execution_count'}) do
      assert(vim.deep_equal(cell[key], after_delete.cells[index][key]), 'deletion and reload changed ' .. key)
    end
  end
  before_empty = after_delete
  local emptied = ui.positions(saved_buf)[1]
  vim.api.nvim_buf_set_lines(saved_buf, emptied.begin[1], emptied['end'][1] + 1, false, {})
  assert(ui.positions(saved_buf)[1].source == '', 'deleting the body included its closing fence')
  assert(#require('ipynb.cells').executable(saved_buf) == 2, 'emptying a cell changed its executable ordinal')
  local empty_formatter = vim.api.nvim_create_autocmd('BufWritePre', {pattern='*.py', callback=function(event)
    for _, line in ipairs(vim.api.nvim_buf_get_lines(event.buf, 0, -1, false)) do
      assert(not line:match('^```'), 'Python formatter received a closing Markdown fence')
    end
  end})
  vim.cmd('write')
  vim.cmd('write')
  vim.api.nvim_del_autocmd(empty_formatter)
  local after_empty = vim.json.decode(table.concat(vim.fn.readfile(relative), '\n'))
  assert(#before_empty.cells == #after_empty.cells, 'emptying a cell lost notebook cells')
  for index, cell in ipairs(before_empty.cells) do
    for _, key in ipairs({'id', 'metadata', 'outputs', 'execution_count'}) do
      assert(vim.deep_equal(cell[key], after_empty.cells[index][key]), 'emptying a cell changed ' .. key)
    end
  end
  assert(table.concat(after_empty.cells[2].source) == '', 'empty source was not saved')
  vim.cmd('edit!')
  assert(#require('ipynb.cells').executable(saved_buf) == 2 and ui.positions(saved_buf)[1].source == '',
    'reopening an emptied cell lost its output anchor')
  local pixels, image_rows = 8, 8
  package.loaded['ipynb.molten_load_image_nvim'] = {image_api={
    from_file=function(_, opts) return opts.id end,
    image_size=function() return {width=20, height=image_rows} end,
    clear=function() end, render=function() end, refresh=function() end,
    layout_key=function() return {pixels, pixels * 2} end,
  }}
  ui.configure({image_provider='image.nvim', image_location='both'})
  ui.sync(saved_buf, {{id='font-resize',begin=span.begin,['end']=position['end'],status='done',
    segments={{kind='image',path='/resize.png'}}}})
  local next_rows
  vim.api.nvim_create_autocmd('VimResized', {callback=function()
    pixels, image_rows = pixels + 4, next_rows
  end})
  for _, rows in ipairs({4, 12, 6}) do
    next_rows = rows
    vim.api.nvim_exec_autocmds('VimResized', {})
    assert(vim.wait(1000, function()
      local marks = vim.api.nvim_buf_get_extmarks(saved_buf, output_ns, 0, -1, {details=true})
      return #marks == 1 and #marks[1][4].virt_lines == rows + 3
    end, 5), 'font resize must remeasure image space after the provider updates cell pixels')
  end
  assert(ui.show('font-resize'))
  local float_buf
  for _, win in ipairs(vim.api.nvim_list_wins()) do
    local displayed = vim.api.nvim_win_get_buf(win)
    if vim.bo[displayed].filetype == 'ipynb_output' then float_buf = displayed end
  end
  assert(float_buf and vim.api.nvim_buf_line_count(float_buf) == 7)
  next_rows = 3
  vim.api.nvim_exec_autocmds('VimResized', {})
  assert(vim.wait(1000, function() return vim.api.nvim_buf_line_count(float_buf) == 4 end, 5),
    'font resize must remeasure an open output float too')
end, debug.traceback)
result.error = not ok and err or nil
vim.fn.writefile({vim.json.encode(result)}, vim.env.IPYNB_VIEW_REPORT)
"#).unwrap();
    let output = Command::new("nvim")
        .args(["--cmd", r#"lua vim.g.ipynb_test_rpcs = {}; local send = vim.fn.chansend; vim.fn.chansend = function(channel, data) local ok, request = pcall(vim.json.decode, data); if ok and type(request) == 'table' and request.method then local counts = vim.g.ipynb_test_rpcs; counts[request.method] = (counts[request.method] or 0) + 1; vim.g.ipynb_test_rpcs = counts end; return send(channel, data) end"#])
        .args(["--headless", "-n", "-i", "NONE", "-u"])
        .arg(
            std::env::var_os("IPYNB_TEST_INIT")
                .map(Into::into)
                .unwrap_or_else(|| root.join("tests/minimal.lua")),
        )
        .arg(&path)
        .arg("-c")
        .arg(format!(
            "lua dofile({})",
            serde_json::to_string(&script).unwrap()
        ))
        .args(["-c", "qa!"])
        .env("IPYNB_VIEW_REPORT", &report)
        .output()
        .unwrap();
    let observed: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(observed["error"].is_null(), "{observed}");
    assert_eq!(
        observed["initial_fences"].as_array().unwrap().len(),
        6,
        "{observed}"
    );
    assert_eq!(
        observed["initial_fences"], observed["recovered_fences"],
        "{observed}"
    );
    assert_eq!(observed["source_unchanged"], true);
    assert_eq!(observed["raw_metadata"], true);
    assert_eq!(
        observed["output_opened"], true,
        "output did not open from the label: {observed}"
    );
    assert_eq!(observed["bottom_opened"], true, "{observed}");
    assert_eq!(
        observed["executed_cells"], 2,
        "prose was executed: {observed}"
    );
    for size in ["wide", "narrow"] {
        let marks = observed[size]["marks"].as_array().unwrap();
        assert!(
            marks.iter().all(|m| m[3]["conceal_lines"].is_null()),
            "concealed metadata traps cursor movement: {observed}"
        );
        let text: String = marks
            .iter()
            .filter_map(|m| m[3]["virt_text"].as_array())
            .flatten()
            .filter_map(|chunk| chunk[0].as_str())
            .collect();
        assert!(
            text.contains('╭') && text.contains('╮') && text.contains('╰') && text.contains('╯'),
            "{observed}"
        );
        for edge in observed[size]["edges"].as_array().unwrap() {
            assert_eq!(edge["width"], observed[size]["width"], "{observed}");
        }
    }
    assert_eq!(
        observed["cells"][0]["lines"],
        json!(["42"]),
        "cell label did not run: {observed}"
    );
    assert!(
        observed["cells"][1]["execution_count"].is_null(),
        "{observed}"
    );
    for action in ["above", "all", "visual", "advanced"] {
        assert_eq!(observed[action], true, "{action}: {observed}");
    }
    let output_text = observed["output"].to_string();
    assert!(
        output_text.contains('╮') && output_text.contains('╯'),
        "{observed}"
    );
    assert_eq!(fs::read(path).unwrap(), original);
}
