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
  assert((vim.g.ipynb_test_rpcs.code_spans or 0) == 0, 'opening repeated the preview span conversion')
  result.open_rpcs = vim.g.ipynb_test_rpcs
  local executable = require('ipynb.cells').executable(buf)
  assert(#executable == 2 and #require('ipynb.cells').spans(buf) == 3, 'preview spans include prose code fences')
  assert((vim.g.ipynb_test_rpcs.code_spans or 0) == 0, 'preview spans were not cached')
  vim.wo.number, vim.wo.relativenumber = true, true
  local function gutter() return {vim.wo.number, vim.wo.relativenumber, vim.wo.statuscolumn} end
  local initial_gutter = gutter()
  local markdown = require('ipynb.markdown')
  local highlighter = assert(vim.treesitter.highlighter.active[buf])
  local highlight_query = highlighter:get_query('markdown'):query()
  local tree = assert(vim.treesitter.get_parser(buf, 'markdown'):parse()[1])
  for capture, _, metadata in highlight_query:iter_captures(tree:root(), buf) do
    assert(not metadata.conceal_lines and not (metadata[capture] or {}).conceal_lines,
      'Markdown highlights hide fence rows and their anchored output')
  end
  assert(vim.treesitter.query.get('markdown', 'highlights').has_conceal_line,
    'notebook highlighting must not change other Markdown buffers')
  local namespace = vim.api.nvim_get_namespaces()['ipynb-markdown']
  local function syntax() return vim.fn.execute('syntax list IpyNotebookCodeFence') end
  result.initial_syntax = syntax()
  local before = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  vim.cmd('syntax clear')
  markdown.enable(buf)
  result.recovered_syntax = syntax()
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
    insert_visible = insert_visible and parses == 1
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
  if result.output_opened then vim.cmd('IpynbHideOutput') end
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
  assert(vim.bo[saved_buf].buftype == '' and vim.b[saved_buf].autoformat == nil,
    'notebooks must inherit normal save and autoformat settings')
  local format_calls = 0
  local format_group = vim.api.nvim_create_augroup('NotebookConfiguredFormatter', {})
  vim.api.nvim_create_autocmd('BufWritePre', {group=format_group, pattern='*.ipynb', callback=function(event)
    local text = vim.api.nvim_buf_get_lines(event.buf, 0, -1, false)
    vim.api.nvim_buf_set_lines(event.buf, 0, -1, false, text)
  end})
  vim.api.nvim_create_autocmd('BufWritePre', {group=format_group, pattern='*.py', callback=function(event)
    assert(vim.bo[event.buf].filetype == 'python', 'code formatter received the wrong filetype')
    format_calls = format_calls + 1
    local text = vim.api.nvim_buf_get_lines(event.buf, 0, -1, false)
    for index, line in ipairs(text) do text[index] = line:gsub('print%(  44  %)', 'print(44)') end
    vim.api.nvim_buf_set_lines(event.buf, 0, -1, false, text)
  end})
  local span = require('ipynb.cells').executable(saved_buf)[1]
  vim.api.nvim_buf_set_lines(saved_buf, span['end'][1] + 1, span['end'][1] + 1, false, {'print(  44  )'})
  vim.api.nvim_exec_autocmds('TextChanged', {buffer=saved_buf})
  local position = ui.positions(saved_buf)[1]
  assert(position.source:find('print(  44  )', 1, true), 'paste was excluded from the cell source')
  for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(saved_buf, output_ns, 0, -1, {details=true})) do
    assert(mark[2] > position['end'][1], 'paste left output inside the code fence')
  end
  vim.cmd('write')
  vim.cmd('write')
  assert(format_calls == 4, 'each save must use configured Python save hooks for both code cells')
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
    assert!(
        observed["initial_syntax"]
            .as_str()
            .unwrap()
            .contains("conceal"),
        "{observed}"
    );
    assert!(
        observed["recovered_syntax"]
            .as_str()
            .unwrap()
            .contains("conceal"),
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
