use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

#[test]
fn panel_callbacks_require_a_visible_live_spec() {
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
local pending, created, rendered, cleared = {}, {}, {}, {}
local api = {
  panel_supported = function() return true end,
  from_file = function(file, opts)
    created[#created + 1] = {file=file, opts=vim.deepcopy(opts)}
    return 'panel-' .. #created
  end,
  render = function(id) rendered[#rendered + 1] = id end,
  clear = function(id) cleared[#cleared + 1] = id end,
}
package.preload['ipynb.molten_load_image_nvim'] = function() return {image_api=api} end
package.preload['image.utils'] = function()
  return {term={get_size=function() return {cell_width=8, cell_height=16} end}}
end
package.preload['ipynb.image_style'] = function()
  return {
    metrics=function() return 16, 2 end,
    generate=function(_, command, done)
      pending[#pending + 1] = {command=vim.deepcopy(command), done=done}
    end,
  }
end
local defer = vim.defer_fn
vim.defer_fn = function() end
local panels = require('ipynb.panels')
local buf = vim.api.nvim_create_buf(false, true)
local source = {}
for i = 1, 80 do source[i] = 'notebook source ' .. i end
vim.api.nvim_buf_set_lines(buf, 0, -1, false, source)
local win = vim.api.nvim_open_win(buf, true, {relative='editor', row=2, col=3, width=40, height=8})
vim.wo[win].wrap, vim.wo[win].number, vim.wo[win].relativenumber = false, false, false
vim.wo[win].signcolumn, vim.wo[win].foldcolumn, vim.wo[win].scrolloff = 'no', '0', 0
vim.api.nvim_set_hl(0, 'IpyNotebookCodeBorder', {fg=0x112233})
vim.cmd('redraw')
panels.output('test-panel', {buf=buf, win=win, first=0, last=3, width=32, group='IpyNotebookCodeBorder'})
panels.refresh()
assert(#pending == 1, 'visible panel must queue generation')
vim.api.nvim_win_set_cursor(win, {40, 0})
vim.fn.winrestview({topline=40, lnum=40, col=0})
vim.cmd('redraw')
assert(vim.fn.screenpos(win, 1, 1).row == 0, 'panel must be outside the viewport')
panels.refresh()
assert(#pending == 1, 'invisible panel must not queue generation')
pending[1].done('stale.png')
assert(#created == 0 and #rendered == 0, 'invisible panel callback must not create or render an image')

vim.api.nvim_win_set_cursor(win, {1, 0})
vim.fn.winrestview({topline=1, lnum=1, col=0})
vim.cmd('redraw')
panels.refresh()
assert(#pending == 2)
pending[2].done('visible.png')
assert(#created == 1 and #rendered == 1, 'unchanged visible request must render')
panels.refresh()
assert(#pending == 2, 'unchanged panel geometry must not queue another image')
vim.api.nvim_win_set_cursor(win, {2, 0})
vim.fn.winrestview({topline=2, lnum=2, col=0})
vim.cmd('redraw')
panels.refresh()
assert(#pending == 3)
assert(vim.deep_equal(pending[2].command, pending[3].command), 'scrolling must reuse the full border bitmap')
-- BufReadCmd temporarily empties the buffer while its converter is running.
vim.api.nvim_buf_set_lines(buf, 0, -1, false, {''})
panels.refresh()
pending[3].done('during-reload.png')
assert(#created == 1 and #rendered == 1, 'reload must invalidate old source ranges')
vim.api.nvim_buf_set_lines(buf, 0, -1, false, source)
vim.cmd('redraw')
panels.refresh()
assert(#pending == 4)
panels.clear_all()
pending[4].done('after-shutdown.png')
panels.refresh()
assert(#created == 1 and #rendered == 1 and #pending == 4, 'shutdown must invalidate pending requests')
assert(vim.deep_equal(vim.api.nvim_buf_get_lines(buf, 0, -1, false), source), 'panel rendering must preserve notebook source')
vim.fn.writefile({vim.json.encode({created=created, rendered=rendered, cleared=cleared,
  command=pending[2].command, buf=buf, win=win})}, vim.env.IPYNB_TEST_REPORT)

-- Exercise the real adapter without requiring an installed image.nvim.
vim.defer_fn = defer
package.loaded['ipynb.molten_load_image_nvim'] = nil
package.preload['ipynb.molten_load_image_nvim'] = nil
package.loaded['image.utils'].window = {get_window=function() end}
package.loaded['ipynb.image_terminal'] = {attach=function() end}
package.loaded['ipynb.image_kitty'] = {track=function() end}
package.loaded['ipynb.image_style'] = {
  metrics=function() return 16, 2, 12 end,
  rounded=function(path, _, _, _, _, _, done) done(path) end,
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
local before, clear_before = placements, clears
vim.cmd('tabnew')
vim.cmd('redraw')
plot_api.refresh()
assert(vim.wait(1000, function() return clears > clear_before end, 5), 'hidden plot must clear')
assert(placements == before and not plot.is_rendered and plot.ipynb_active,
  'hidden-tab plot must not render or lose its active state')
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

local framed = plot_api.from_file('frame.png', {id='stable-frame', buffer=buf, window=win,
  frame={x=3,y=2,width=8,height=3,bounds={left=3,right=11,top=2,bottom=4}}})
local before_frame = placements
plot_api.render(framed)
assert(vim.wait(1000, function() return placements > before_frame end, 5))
local frame, placed = plot, placements
frame:render(); frame:render()
assert(placements == placed, 'unchanged pixel borders must not resend Kitty placements')
frame:clear(); frame:render()
assert(placements == placed + 1, 'a cleared border must still render again')
local toast = vim.api.nvim_open_win(popup_buf, false, {
  relative='editor', row=7, col=5, width=10, height=1, zindex=200,
})
local clear_toast = clears
frame:render()
assert(clears == clear_toast and frame.is_rendered, 'distant notifications must not hide an entire notebook')
vim.api.nvim_win_close(toast, true)
local covering = vim.api.nvim_open_win(popup_buf, false, {
  relative='editor', row=2, col=3, width=4, height=2, zindex=200, noautocmd=true,
})
vim.cmd('redraw')
assert(vim.wait(1000, function() return not frame.is_rendered end, 5),
  'popups that suppress window events must remain above pixel borders')
vim.api.nvim_win_set_config(covering, {hide=true})
vim.cmd('redraw')
assert(vim.wait(1000, function() return frame.is_rendered end, 5),
  'hiding a popup without window events must restore notebook graphics')
vim.api.nvim_win_close(covering, true)
frame:render()
assert(frame.is_rendered, 'closing the overlapping popup must restore the border')

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
    assert_eq!(
        observed["created"],
        json!([{"file":"visible.png", "opts":{
            "id":format!("test-panel:{}", observed["win"]),
            "buffer":observed["buf"], "window":observed["win"],
            "frame":{"x":3,"y":2,"width":32,"height":4,"full_height":4,"crop":0,
                "bounds":{"left":3,"right":35,"top":2,"bottom":5}}
        }}])
    );
    assert_eq!(observed["command"][1], "256x64");
    assert_eq!(observed["rendered"], json!(["panel-1"]));
    assert_eq!(observed["cleared"], json!(["panel-1"]));
}
