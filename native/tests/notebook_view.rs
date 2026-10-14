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
  local raw, late_completion
  vim.defer_fn(function()
    markdown.enable(buf)
    raw = vim.api.nvim_get_mode().mode == 'i'
      and vim.fn.foldclosed(1) == -1
      and #vim.api.nvim_buf_get_extmarks(buf, namespace, 0, -1, {}) == 0
      and #vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces()['ipynb-cell-header'], 0, -1, {}) == 0
    local prose = vim.api.nvim_get_namespaces()['render-markdown.nvim']
    if prose then raw = raw and #vim.api.nvim_buf_get_extmarks(buf, prose, 0, -1, {}) == 0 end
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
  assert(raw, 'Insert mode did not expose raw metadata and fences')
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
  for _, state in ipairs({
    {'hold', true, false, 'Queued', 'Queued'}, {'running', true, false, 'Running', 'Running'},
    {'done', true, false, 'Done', 'Success'}, {'done', false, false, 'Failed', 'Fail'},
    {'done', true, true, 'Previous', 'Stale'},
  }) do
    local snapshot = vim.deepcopy(cells)
    snapshot[1].status, snapshot[1].success, snapshot[1].old = state[1], state[2], state[3]
    ui.sync(buf, snapshot)
    local marks = vim.api.nvim_buf_get_extmarks(buf, output_ns, 0, -1, {details=true})
    local header = marks[1][4].virt_lines[1][1]
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
