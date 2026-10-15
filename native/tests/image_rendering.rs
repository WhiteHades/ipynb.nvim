use serde_json::Value;
use std::{fs, path::Path, process::Command};

#[test]
fn inline_plots_follow_windows_and_preserve_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let base = root.join(".tmp/image-panels");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::tempdir_in(base).unwrap();
    let script = fixture.path().join("check.lua");
    let report = fixture.path().join("report.json");
    fs::write(
        &script,
        r#"
vim.opt.rtp:prepend(vim.env.IPYNB_TEST_ROOT)
local buf = vim.api.nvim_create_buf(false, true)
local source = {}
for i = 1, 80 do source[i] = 'notebook source ' .. i end
vim.api.nvim_buf_set_lines(buf, 0, -1, false, source)
local win = vim.api.nvim_open_win(buf, true, {relative='editor', row=2, col=3, width=40, height=8})
vim.wo[win].wrap, vim.wo[win].number, vim.wo[win].relativenumber = false, false, false
vim.wo[win].signcolumn, vim.wo[win].foldcolumn, vim.wo[win].scrolloff = 'no', '0', 0
vim.cmd('redraw')
package.loaded['image.utils'] = {
  term={get_size=function() return {cell_width=8,cell_height=16} end},
  window={get_window=function() end},
}
package.loaded['ipynb.image_terminal'] = {attach=function() end}
local tmux_offset, tmux_notify, tmux_unwatched = 0, nil, 0
package.loaded['ipynb.image_tmux'] = {
  top_offset=function() return tmux_offset end,
  watch=function(callback) tmux_notify=callback; return true end,
  unwatch=function() tmux_unwatched=tmux_unwatched+1 end,
}
package.loaded['ipynb.image_kitty'] = {track=function() end}
local style_releases = 0
package.loaded['ipynb.image_style'] = {
  metrics=function() return 16, 2, 12 end,
  rounded=function(path, _, _, _, _, _, done) done(path, function() style_releases = style_releases + 1 end) end,
}
local placements, clears, plot = 0, 0
local state = {enabled=true, options={}, backend={render=function(img)
  placements = placements + 1
  img.is_rendered = true
end}}
package.loaded.image = {from_file=function()
  plot = {geometry={}, image_width=160, image_height=80, global_state=state,
    render=function() error('prepared plot must use the backend directly') end,
    clear=function(self) clears = clears + 1; self.is_rendered = false end}
  return plot
end}
local plot_api = require('ipynb.molten_load_image_nvim').image_api
plot_api.panel_supported = function() return true end
local id = plot_api.from_file('plot.png', {
  id='tab-plot', buffer=buf, window=win, x=2, y=0, max_width=20, max_height=5,
})
plot_api.image_size(id)
plot_api.render(id)
assert(vim.wait(1000, function() return placements > 0 end, 5), 'visible plot must render')
vim.wait(20)
local schedule, pending = vim.schedule, {}
vim.schedule = function(callback) pending[#pending + 1] = callback end
for _ = 1, 100 do
  vim.api.nvim_exec_autocmds('WinScrolled', {group='ipynb_image_placements', modeline=false})
end
vim.schedule = schedule
assert(#pending == 1, 'scroll events must share one pending image frame, not queue another refresh each')
local before_scroll, clear_before_scroll = placements, clears
pending[1]()
assert(placements > before_scroll, 'the next image frame must apply the current placement')
assert(clears == clear_before_scroll, 'scroll callbacks must not pass their event table as a forced refresh')
local visible_placements, visible_clears = placements, clears
assert(style_releases == 0, 'accepted styled images must retain their cache lease while visible')
tmux_offset = nil
tmux_notify()
assert(vim.wait(1000, function() return clears > visible_clears end, 5), 'tmux notification must clear hidden plots without focus events')
assert(placements == visible_placements and not plot.is_rendered and plot.ipynb_active)
assert(style_releases == 0, 'visibility clears must retain the styled image lease')
tmux_offset = 1
tmux_notify()
assert(vim.wait(1000, function() return placements > visible_placements end, 5), 'tmux notification must restore visible plots')
local before, clear_before = placements, clears
vim.cmd('tabnew')
vim.cmd('redraw')
plot_api.refresh()
assert(vim.wait(1000, function() return clears > clear_before end, 5), 'hidden plot must clear')
assert(placements == before and not plot.is_rendered and plot.ipynb_active,
  'hidden-tab plot must not render or lose its active state')
assert(style_releases == 0, 'tab visibility changes must retain the styled image lease')
vim.cmd('tabprevious')
vim.cmd('redraw')
assert(vim.wait(1000, function() return placements > before end, 5), 'TabEnter must restore the active plot')

local popup_buf = vim.api.nvim_create_buf(false, true)
local before_popup, before_clear = placements, clears
local popup = vim.api.nvim_open_win(popup_buf, false, {
  relative='editor', row=3, col=4, width=10, height=3, zindex=200, border='rounded',
})
vim.bo[popup_buf].filetype = 'which_key'
vim.cmd('redraw')
assert(vim.wait(1000, function() return clears > before_clear end, 5),
  'opening WhichKey must hide terminal graphics without moving the notebook cursor')
plot:render()
assert(placements == before_popup and not plot.is_rendered, 'popup must stay above async image redraws')
vim.api.nvim_win_close(popup, true)
assert(vim.wait(1000, function() return placements > before_popup end, 5), 'closing popup must restore graphics')

local toast = vim.api.nvim_open_win(popup_buf, false, {
  relative='editor', row=3, col=35, width=4, height=1, zindex=200,
})
local clear_toast = clears
plot:render()
assert(clears == clear_toast and plot.is_rendered, 'distant notifications must not hide an entire notebook')
vim.api.nvim_win_close(toast, true)
local covering = vim.api.nvim_open_win(popup_buf, false, {
  relative='editor', row=3, col=5, width=4, height=2, zindex=200, noautocmd=true,
})
vim.cmd('redraw')
assert(vim.wait(1000, function() return not plot.is_rendered end, 5),
  'popups that suppress window events must remain above plots')
vim.api.nvim_win_set_config(covering, {hide=true})
vim.cmd('redraw')
assert(vim.wait(1000, function() return plot.is_rendered end, 5),
  'hiding a popup without window events must restore plots')
vim.api.nvim_win_close(covering, true)
plot:render()
assert(plot.is_rendered, 'closing the overlapping popup must restore the plot')
assert(vim.deep_equal(vim.api.nvim_buf_get_lines(buf, 0, -1, false), source), 'image rendering must preserve source')

local medium = plot_api.from_file('medium.png', {
  id='medium', buffer=buf, window=win, render_offset_top=1, max_width=80, max_height=40,
})
plot.ipynb_source_width, plot.ipynb_source_height = 4000, 3000
for _, pixels in ipairs({8, 16, 6}) do
  package.loaded['image.utils'].term.get_size = function() return {cell_width=pixels,cell_height=pixels*2} end
  local size = plot_api.image_size(medium)
  assert(size.width > 0 and size.width <= 40 and size.height > 0 and size.height <= 14,
    'inline plots must remain medium sized at different font sizes')
  local aspect = size.width / (size.height * 2)
  assert(math.abs(aspect - 4/3) < 0.1, 'font resize must preserve plot aspect ratio within cell rounding')
end

local screenpos_calls, offscreen_rows_measured, clear_before, render_before = 0, 0, clears, placements
local screenpos = vim.fn.screenpos
vim.fn.screenpos = function(...)
  screenpos_calls = screenpos_calls + 1
  local _, line = ...
  if line and line >= 30 and line <= 69 then offscreen_rows_measured = offscreen_rows_measured + 1 end
  return screenpos(...)
end
for row = 30, 69 do
  local id = plot_api.from_file('offscreen-' .. row .. '.png', {
    id='offscreen-' .. row, buffer=buf, window=win, x=2, y=row, max_width=20, max_height=5,
  })
  plot_api.image_size(id)
  plot_api.render(id)
end
assert(vim.wait(1000, function() return clears >= clear_before + 40 end, 5), 'offscreen plots must be cleared')
local refresh_screenpos_calls = screenpos_calls
local refresh_clear_calls, refresh_render_calls = clears - clear_before, placements - render_before
assert(offscreen_rows_measured == 0, 'offscreen plots must be culled before screen position measurement')
vim.fn.screenpos = screenpos
local release_before_clear = style_releases
plot_api.clear('tab-plot')
assert(style_releases == release_before_clear + 1, 'lifecycle clear must release the accepted style lease')
plot_api.clear_all()
assert(vim.wait(1000, function() return tmux_unwatched == 1 end, 5), 'last-image cleanup must release the tmux watcher once')

local ui = require('ipynb.native_ui')
vim.g.ipynb_image_provider = 'none'
ui.configure({image_provider='none', virt_lines_off_by_1=true})
local notebook = vim.api.nvim_create_buf(false, true)
vim.api.nvim_set_current_buf(notebook)
vim.api.nvim_buf_set_lines(notebook, 0, -1, false, {'```python', '', '```', '', 'prose'})
ui.sync(notebook, {{id=1, begin={1,0}, ['end']={1,0}, status='new', segments={}}})
-- Normal-mode linewise paste appends after the empty cell's original end.
vim.api.nvim_buf_set_lines(notebook, 2, 2, false, {'print(42)', string.rep('x', 200)})
ui.refresh(notebook)
local position = ui.positions(notebook)[1]
assert(vim.deep_equal(position['end'], {3,200}), 'pasted lines must extend the executable cell')
assert(position.source == '\nprint(42)\n' .. string.rep('x',200), 'execution must include all pasted text')
local output_ns = vim.api.nvim_get_namespaces()['ipynb-native-output']
local marks = vim.api.nvim_buf_get_extmarks(notebook, output_ns, 0, -1, {details=true})
assert(#marks == 1 and marks[1][2] == 4, 'output must stay below the closing fence')
vim.api.nvim_buf_set_lines(notebook, 2, 4, false, {})
ui.refresh(notebook)
assert(vim.deep_equal(ui.positions(notebook)[1]['end'], {1,0}), 'deleting pasted lines must shrink the cell')
vim.api.nvim_buf_set_lines(notebook, 1, 1, false, {'print(0)'})
assert(vim.deep_equal(ui.positions(notebook)[1].begin, {1,0}), 'paste before the first line must remain inside the cell')
ui.clear(notebook)
vim.api.nvim_buf_set_lines(notebook, 0, -1, false, {'```python', 'print(1)', 'print(2)', '```'})
vim.b[notebook].ipynb_code_spans = {{begin={1,0}, ['end']={2,8}}}
vim.b[notebook].ipynb_code_spans_tick = vim.api.nvim_buf_get_changedtick(notebook)
ui.sync(notebook, {{id=2, begin={1,0}, ['end']={1,8}, status='done', lines={'1'}}})
assert(vim.deep_equal(ui.positions(notebook)[1]['end'], {1,8}), 'line and visual execution must keep their selection bounds')
marks = vim.api.nvim_buf_get_extmarks(notebook, output_ns, 0, -1, {details=true})
assert(#marks == 1 and marks[1][2] == 3, 'selection output must stay outside the containing code block')
ui.shutdown()
vim.fn.writefile({vim.json.encode({plots_rendered=placements, refresh_screenpos_calls=refresh_screenpos_calls,
  offscreen_rows_measured=offscreen_rows_measured,
  refresh_clear_calls=refresh_clear_calls, refresh_render_calls=refresh_render_calls})}, vim.env.IPYNB_TEST_REPORT)
"#,
    )
    .unwrap();
    let output = Command::new(std::env::var_os("IPYNB_TEST_NVIM").unwrap_or_else(|| "nvim".into()))
        .args(["--headless", "-u", "NONE", "-l"])
        .arg(script)
        .env("IPYNB_TEST_ROOT", root)
        .env("IPYNB_TEST_REPORT", &report)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let observed: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    eprintln!("image rendering counters: {observed}");
    assert!(observed["plots_rendered"].as_u64().unwrap() > 0);
}
