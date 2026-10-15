use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

struct TmuxServer(std::path::PathBuf);

impl Drop for TmuxServer {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.0)
            .args(["kill-server"])
            .output();
    }
}

fn tmux(socket: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tmux {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn notebook_kitty_placements_preserve_geometry_and_coalesce_frame_completion() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let base = root.join(".tmp/image-kitty");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::tempdir_in(base).unwrap();
    let script = fixture.path().join("check.lua");
    let report = fixture.path().join("report.json");
    fs::write(&script, r#"
vim.opt.rtp:prepend(vim.env.IPYNB_TEST_ROOT)
local writes, sent, deferred, counts, loads, finishes, graphics_fallbacks = {}, {}, {}, {}, 0, 0, {}
vim.api.nvim_list_uis = function() return {{stdout_tty=true}} end
vim.api.nvim_ui_send = function(value) sent[#sent + 1] = value end
local completions = {}
local helpers = {
  write = function() end,
  write_graphics_at = function(config, x, y)
    writes[#writes + 1] = {config=vim.deepcopy(config), x=x, y=y}
    return 'sent'
  end,
  write_graphics = function(config) graphics_fallbacks[#graphics_fallbacks + 1] = config; return 'delegated' end,
  update_sync_end = function() finishes = finishes + 1 end,
}

package.preload['image/backends/kitty/helpers'] = function() loads = loads + 1; return helpers end
package.preload['image/backends/kitty/codes'] = function() return {control={keys={action='a',image_id='i',transmit_medium='t',display_width='w',display_height='h',display_x='x',display_y='y',display_columns='c',display_rows='r',display_zindex='z'},transmit_medium={file='f'}}} end
package.preload['image/utils/tmux'] = function() return {is_tmux=false,escape=function(value) return value end} end
vim.defer_fn = function(callback, delay) deferred[#deferred + 1] = {callback=callback, delay=delay} end
vim.schedule = function(callback) completions[#completions + 1] = callback end
local cold = package.loaded['image/backends/kitty/helpers'] == nil
local bridge = require('ipynb.image_kitty')
local img = {internal_id=7, global_state={options={backend='kitty'}}}
bridge.track(img, {get_size=function() return {cell_width=8, cell_height=16} end})
bridge.track(img, {get_size=function() return {cell_width=8, cell_height=16} end})
local config = {action='p', image_id=7, display_width=320, display_height=320, display_x=8, display_y=16}
local result = helpers.write_graphics_at(config, 4, 6)
helpers.write_graphics({action='t', image_id=7, transmit_medium='f'}, '/tmp/a.png', 4)
helpers.write_graphics({action='d', image_id=7})
helpers.write_graphics({action='t', image_id=7, transmit_medium='d'}, '/tmp/direct.png')
counts[#counts + 1] = #deferred
local other = vim.tbl_extend('force', config, {image_id=8})
helpers.write_graphics_at(other, 1, 2)
counts[#counts + 1] = #deferred
helpers.write_graphics_at({action='d', image_id=7}, 0, 0)
counts[#counts + 1] = #deferred
helpers.write_graphics_at({action='p', image_id=7}, 3, 4)
counts[#counts + 1] = #deferred
local delays, before = {}, finishes
for _, item in ipairs(deferred) do delays[#delays + 1] = item.delay; item.callback() end
assert(#completions == 1, 'placements must share one scheduled frame completion')
for _, callback in ipairs(completions) do callback() end
package.loaded['ipynb.image_kitty'] = nil
package.loaded['image/backends/kitty/helpers'] = {}
local warnings = {}
vim.notify = function(message) warnings[#warnings + 1] = message end
local unsupported = require('ipynb.image_kitty')
local survived, failure = pcall(function() unsupported.track(img, {}); unsupported.track(img, {}) end)
vim.fn.writefile({vim.json.encode({cold=cold, loads=loads, writes=writes, sent=sent, graphics_fallbacks=graphics_fallbacks, original=config,
  other=other, result=result, counts=counts, delays=delays, before=before, finishes=finishes,
  survived=survived, failure=failure, warnings=warnings, wire_id=img.ipynb_wire_id})}, vim.env.IPYNB_TEST_REPORT)
-- Keep tmux variants in this transport smoke check, not a separate Lua runner.
vim.api.nvim_list_uis = function() return {} end
package.loaded['ipynb.image_kitty'], package.loaded['ipynb.image_tmux'] = nil, nil
local tty, tty_queries = '/dev/pts/1', 0
local tmux, receiver_queries, scheduled = {is_tmux=true, get_pane_tty=function()
  tty_queries = tty_queries + 1
  return tty
end}, 0, {}
local offset_queries, live_queries = 0, 0
local live_rows = '101 55 /dev/pts/1 @2 0\n202 66 /dev/pts/2 @2 0\n303 77 /dev/pts/3 @2 1\n'
local reply = '1 1 1 top on 40 0 0 0\n'
package.preload['image/utils/tmux'] = function() return tmux end
package.loaded['image/utils/tmux'] = nil
local last, count = nil, 0
local tmux_graphics_fallbacks, raw_writes = {}, {}
helpers = {update_sync_end=function() end, write=function(sequence, destination, flush)
  raw_writes[#raw_writes + 1] = {sequence=sequence, tty=destination, flush=flush}
end, write_graphics=function(value, data, chunk_size)
  tmux_graphics_fallbacks[#tmux_graphics_fallbacks + 1] = {config=vim.deepcopy(value), data=data, chunk_size=chunk_size}
  return 'delegated'
end, write_graphics_at=function(value, x, y)
  last = {config=value, x=x + (tmux.is_tmux and 12 or 0), y=y + (tmux.is_tmux and 7 or 0)}
  count = count + 1
  return 'sent'
end}
package.loaded['image/backends/kitty/helpers'] = helpers
vim.env.TMUX_PANE = '%42'
vim.fn.system = function(command)
  assert(command[1] == 'tmux')
  if command[2] == 'display-message' and command[6] == '#{window_id}' then
    receiver_queries = receiver_queries + 1
    return '@1\n101 55 /dev/pts/1 @1 0\n202 66 /dev/pts/2 @2 0\n303 77 /dev/pts/3 @1 1\n404 88 /tmp/pts @1 0\nmalformed client row\n'
  end
  if command[2] == 'list-clients' then live_queries = live_queries + 1; return live_rows end
  assert(command[2] == 'display-message' and command[4] == '-t' and command[5] == '%42')
  offset_queries = offset_queries + 1
  return reply
end
vim.schedule = function(callback) scheduled[#scheduled + 1] = callback end
local function batch(value)
  for _, callback in ipairs(scheduled) do callback() end
  scheduled, reply = {}, value or reply
end
require('ipynb.image_kitty').track(img, {get_size=function() return {cell_width=8,cell_height=16} end})
local function place(expected)
  local previous = count
  helpers.write_graphics_at(config, 4, 6)
  assert(count == previous + (expected and 1 or 0), 'hidden placement escaped')
  if expected then
    assert(last.x == 16 and last.y == expected, vim.inspect(last))
    assert(last.config.display_columns == 40 and last.config.display_rows == 20)
  end
end
place(14); place(14)
assert(offset_queries == 1, 'source-window offset lookups: ' .. offset_queries)
assert(receiver_queries == 1, 'visible-client receiver lookups: ' .. receiver_queries)
assert(tmux.get_pane_tty() == '/dev/pts/1' and tmux.get_pane_tty() == '/dev/pts/1')
assert(tty_queries == 1, 'clears share the active pane lookup within a batch')
batch(); tty = '/dev/pts/2'
assert(tmux.get_pane_tty() == '/dev/pts/2' and tty_queries == 2, 'pane switches invalidate the clear destination')
for _, case in ipairs({{'bottom on 40',13},{'top off 40',13},{'top 2 40',15},
  {'top 0 40',13},{'top 1 40',14},{'bottom 2 40',13},{'top 2 2',13},{'top 2 1',13}}) do
  batch('1 1 1 ' .. case[1] .. ' 0 0 0\n'); place(case[2])
end
batch('1 0 1 top on 40 0 0 0\n'); place(14)
batch('1 1 1 top on 40 0 1 0\n'); place(nil)
batch('1 1 1 top on 40 0 1 1\n'); place(14)
for _, hidden in ipairs({'0 1 1 top on 40 0 0 0','1 1 0 top on 40 0 0 0',
  '1 1 1 top on 40 1 0 0','malformed','1 1 1 top 9 40 0 0 0'}) do
  batch(hidden); place(nil)
end
batch('1 1 1 top 2 40 0 0 0'); place(15)
img.global_state.disable_decorator_handling = true; place(nil)
img.global_state.disable_decorator_handling = false
batch(); vim.env.TMUX_PANE = nil
local before_queries = receiver_queries + offset_queries
place(nil); assert(receiver_queries + offset_queries == before_queries)
batch(); vim.env.TMUX_PANE = '%42'; tmux.is_tmux = false
helpers.write_graphics_at(config, 4, 6)
assert(last.x == 4 and last.y == 6 and receiver_queries + offset_queries == before_queries)
tmux.is_tmux = true
helpers.write_graphics_at(other, 4, 6)
assert(last.x == 16 and last.y == 13 and receiver_queries + offset_queries == before_queries)
-- Exercise the serialized UI path with a tmux status-row offset and passthrough escaping.
tmux.get_pane_position = function() return {left=12, top=7} end
tmux.escape = function(value) return '\27Ptmux;' .. value:gsub('\27', '\27\27') .. '\27\\' end
vim.api.nvim_list_uis = function() return {{stdout_tty=true}} end
package.loaded['ipynb.image_kitty'], package.loaded['ipynb.image_tmux'] = nil, nil
package.loaded['image/backends/kitty/helpers'] = helpers
reply = '1 1 1 top on 40 0 0 0\n'
local transport_img = {internal_id=7, global_state={options={backend='kitty'}}}
require('ipynb.image_kitty').track(transport_img, {get_size=function() return {cell_width=8,cell_height=16} end})
local second_img = {internal_id=9, global_state={options={backend='kitty'}}}
require('ipynb.image_kitty').track(second_img, {get_size=function() return {cell_width=8,cell_height=16} end})
assert(transport_img.ipynb_wire_id ~= transport_img.internal_id
  and transport_img.ipynb_wire_id ~= second_img.ipynb_wire_id, 'images need independent private wire IDs')
local before_sent = #sent
helpers.write_graphics_at(config, 4, 6)
assert(#sent == before_sent + 1 and sent[#sent]:find('\27Ptmux;') == 1, vim.inspect(sent))
assert(sent[#sent]:find('\27\27%[14;16H') ~= nil, vim.inspect(sent[#sent]))
assert(sent[#sent]:find('i=' .. transport_img.ipynb_wire_id, 1, true), 'placement must use the private Kitty wire ID')
assert(transport_img.internal_id == 7 and config.image_id == 7, 'placement must preserve internal ID and caller config')
vim.fn.writefile({vim.json.encode({cold=cold, loads=loads, writes=writes, sent=sent, graphics_fallbacks=graphics_fallbacks, original=config,
  other=other, result=result, counts=counts, delays=delays, before=before, finishes=finishes,
  survived=survived, failure=failure, warnings=warnings, wire_id=img.ipynb_wire_id})}, vim.env.IPYNB_TEST_REPORT)
-- Transmit and hidden cleanup reuse the wire ID and target only original live receivers.
batch()
local transmit_config = {action='t', image_id=7, transmit_medium='f'}
local transmit_start = #sent
helpers.write_graphics(transmit_config, '/tmp/image.png', 8)
assert(transmit_config.image_id == 7 and sent[transmit_start + 1]:find('i=' .. transport_img.ipynb_wire_id, 1, true),
  'transmit must use the wire ID without changing the caller config')
local delete_config = {action='d', image_id=7}
local writes_before, fallback_count = #raw_writes, #tmux_graphics_fallbacks
local sent_count, live_queries_before = #sent, live_queries
vim.env.TMUX_PANE = nil
helpers.write_graphics(delete_config)
assert(#raw_writes == writes_before + 1 and raw_writes[#raw_writes].tty == '/dev/pts/1'
  and raw_writes[#raw_writes].flush == false, 'hidden delete must use the recorded matching receiver TTY')
assert(raw_writes[#raw_writes].sequence:find('i=' .. transport_img.ipynb_wire_id, 1, true),
  'delete must use the same private wire ID')
assert(vim.tbl_count(transport_img.ipynb_tmux_clients) == 1,
  'the placement must record only the matching visible non-control client')
assert(#sent == sent_count and #tmux_graphics_fallbacks == fallback_count,
  'hidden delete must not broadcast or delegate to another TTY')
assert(delete_config.image_id == 7 and transport_img.internal_id == 7)
assert(live_queries == live_queries_before + 1, 'hidden cleanup must validate receivers against live clients')
helpers.write_graphics(delete_config)
assert(live_queries == live_queries_before + 1, 'live client lookup must be shared within the batch')
batch()
live_rows = '202 66 /dev/pts/2 @2 0\n'
writes_before, live_queries_before = #raw_writes, live_queries
helpers.write_graphics(delete_config)
assert(#raw_writes == writes_before and live_queries == live_queries_before + 1,
  'a detached original client must not receive a delete')
batch()
live_rows = '101 56 /dev/pts/1 @2 0\n' -- The same PTY has a new client identity.
writes_before, live_queries_before = #raw_writes, live_queries
helpers.write_graphics(delete_config)
assert(#raw_writes == writes_before and live_queries == live_queries_before + 1,
  'a reused TTY must not receive a stale client delete')
local before_untracked = #tmux_graphics_fallbacks
local queries_before_untracked = receiver_queries + offset_queries + live_queries
helpers.write_graphics({action='d', image_id=99, tty='/dev/pts/destination'})
assert(#tmux_graphics_fallbacks == before_untracked + 1
  and receiver_queries + offset_queries + live_queries == queries_before_untracked,
  'untracked delete must delegate without querying tmux')
"#).unwrap();
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
    assert_eq!(observed["cold"], true);
    assert_eq!(observed["loads"], 1);
    assert_eq!(observed["result"], Value::Null);
    assert_eq!(observed["writes"].as_array().unwrap().len(), 2);
    let original = json!({"action":"p", "image_id":7, "display_width":320,
        "display_height":320, "display_x":8, "display_y":16});
    assert_eq!(observed["original"], original);
    let mut other = original.clone();
    other["image_id"] = json!(8);
    assert_eq!(observed["other"], other);
    assert_eq!(
        observed["writes"][0],
        json!({"config":observed["other"], "x":1, "y":2})
    );
    assert!(
        observed["sent"][0]
            .as_str()
            .unwrap()
            .contains("\u{001b}[6;4H\u{001b}_G")
    );
    let first_header = observed["sent"][1]
        .as_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert!(first_header.starts_with("\u{001b}_G"));
    let wire_id = format!("i={}", observed["wire_id"].as_u64().unwrap());
    for field in ["a=t", wire_id.as_str(), "t=f"] {
        assert!(first_header.contains(field), "{first_header}");
    }
    assert!(!first_header.contains("\u{001b}_GGa="), "{first_header}");
    assert!(observed["sent"][1].as_str().unwrap().contains(";L3Rt"));
    assert!(observed["sent"][2].as_str().unwrap().contains("m=1;cC9h"));
    assert!(observed["sent"][3].as_str().unwrap().contains("m=1;LnBu"));
    assert!(observed["sent"][4].as_str().unwrap().contains("m=0;Zw=="));
    let delete_sequence = observed["sent"][5].as_str().unwrap();
    assert!(delete_sequence.starts_with("\u{001b}_G") && delete_sequence.ends_with("\u{001b}\\"));
    assert!(delete_sequence.contains("a=d"), "{delete_sequence}");
    assert!(delete_sequence.contains(&wire_id), "{delete_sequence}");
    assert!(
        observed["sent"][6]
            .as_str()
            .unwrap()
            .contains("\u{001b}[4;3H\u{001b}_G")
    );
    assert_eq!(observed["graphics_fallbacks"].as_array().unwrap().len(), 1);
    assert_eq!(observed["graphics_fallbacks"][0]["transmit_medium"], "d");
    assert_eq!(observed["sent"].as_array().unwrap().len(), 9); // includes ordered sync completion and tmux placement
    assert_eq!(observed["other"]["display_columns"], Value::Null);
    assert_eq!(observed["counts"], json!([0, 0, 0, 0]));
    assert_eq!(observed["delays"], json!([]));
    assert_eq!(observed["before"], 0);
    assert_eq!(observed["finishes"], 0);
    assert_eq!(observed["survived"], true);
    assert_eq!(observed["warnings"].as_array().unwrap().len(), 1);
    assert!(
        observed["warnings"][0]
            .as_str()
            .unwrap()
            .contains("Update both plugins")
    );
}

#[test]
#[ignore = "requires tmux"]
fn tmux_watch_refreshes_offset_and_removes_only_owned_hooks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let base = root.join(".tmp/image-kitty");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::tempdir_in(base).unwrap();
    let socket = fixture.path().join("tmux.sock");
    let _server = TmuxServer(socket.clone());
    let capture = fixture.path().join("tmux-env");
    let capture_script = fixture.path().join("capture-env.sh");
    let script = fixture.path().join("watch.lua");
    fs::write(
        &capture_script,
        "#!/bin/sh\nprintf '%s\\n%s\\n%s\\n' \"$TMUX\" \"$TMUX_PANE\" \"$(tty)\" > \"$IPYNB_TEST_ENV_CAPTURE\"\nexec sleep 120\n",
    )
    .unwrap();
    let mut start = Command::new("tmux");
    start
        .arg("-S")
        .arg(&socket)
        .args([
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "image-watch",
            "-x",
            "80",
            "-y",
            "24",
            "sh",
        ])
        .arg(&capture_script)
        .env("IPYNB_TEST_ENV_CAPTURE", &capture);
    let output = start.output().unwrap();
    assert!(
        output.status.success(),
        "tmux new-session: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for _ in 0..100 {
        if capture.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let values = fs::read_to_string(&capture).unwrap();
    let mut values = values.lines();
    let tmux_env = values.next().unwrap();
    let pane = values.next().unwrap();
    let tty = values.next().unwrap();
    assert!(tmux_env.starts_with(&socket.to_string_lossy().to_string()));
    assert!(pane.starts_with('%'));
    assert!(tty.starts_with("/dev/"));

    tmux(&socket, &["set-option", "-g", "focus-events", "off"]);
    tmux(&socket, &["set-option", "-g", "status", "on"]);
    tmux(&socket, &["set-option", "-g", "status-position", "bottom"]);
    tmux(
        &socket,
        &[
            "set-hook",
            "-g",
            "session-window-changed[0]",
            "display-message -p preserved-hook",
        ],
    );
    tmux(
        &socket,
        &[
            "new-window",
            "-d",
            "-t",
            "image-watch:1",
            "-P",
            "-F",
            "#{pane_id}",
        ],
    );
    let hooks_before = String::from_utf8(tmux(&socket, &["show-hooks", "-g"]).stdout).unwrap();
    assert!(hooks_before.contains("session-window-changed[0]"));

    fs::write(
        &script,
        r#"
vim.opt.rtp:prepend(vim.env.IPYNB_TEST_ROOT)
package.preload['image/utils/tmux'] = function()
  return {is_tmux=true, get_pane_tty=function() return vim.env.IPYNB_TEST_PANE_TTY end}
end
package.loaded['image/utils/tmux'] = nil
-- This detached Nvim has no tmux client; keep pane/window/status output real and synthesize only client context.
local system = vim.fn.system
vim.fn.system = function(command, ...)
  if type(command) == 'table' and command[1] == 'tmux' and command[2] == 'display-message' then
    command = vim.deepcopy(command)
    local format = #command
    command[format] = command[format]
      :gsub('#{==:#{client_session},#{session_name}}', '1')
      :gsub('#{client_height}', '24')
      :gsub('#{client_control_mode}', '0')
  end
  return system(command, ...)
end
local temp_prefix = vim.env.IPYNB_TEST_TMP .. '/watch'
vim.fn.tempname = function() return temp_prefix end
local image_tmux = require('ipynb.image_tmux')
local callbacks, focus_lost, last_offset = 0, 0, nil
vim.api.nvim_create_autocmd('FocusLost', {callback=function() focus_lost = focus_lost + 1 end})
assert(image_tmux.watch(function()
  callbacks = callbacks + 1
  last_offset = image_tmux.top_offset()
end), 'tmux watcher failed to start')
assert(image_tmux.top_offset() == 0, 'bottom status must add no rows')
local function tmux(...)
  local command = {'tmux', '-S', vim.env.IPYNB_TEST_SOCKET, ...}
  local result = vim.fn.system(command)
  assert(vim.v.shell_error == 0, result)
  return result
end
local function changed(command)
  local previous = callbacks
  tmux(unpack(command))
  assert(vim.wait(5000, function() return callbacks > previous end, 10), 'tmux hook did not wake the uv watcher')
end
changed({'select-window', '-t', 'image-watch:1'})
assert(focus_lost == 0, 'window switch unexpectedly depended on FocusLost')
changed({'select-window', '-t', 'image-watch:0'})
assert(image_tmux.top_offset() == 0, 'returning to the original window must restore the bottom offset')
changed({'set-option', '-g', 'status-position', 'top'})
assert(image_tmux.top_offset() == 1, 'top status must add one row')
tmux('split-window', '-h', '-d', '-t', 'image-watch:0')
local panes = {}
for pane_id in tmux('list-panes', '-t', 'image-watch:0', '-F', '#{pane_id}'):gmatch('[^\r\n]+') do
  panes[#panes + 1] = pane_id
end
assert(#panes == 2, 'split-window must create a sibling pane')
changed({'select-pane', '-t', panes[2]})
assert(last_offset == 1, 'an inactive pane must remain visible beside its unzoomed sibling')
changed({'resize-pane', '-Z', '-t', panes[2]})
assert(last_offset == nil, 'a zoomed sibling must hide the inactive pane')
changed({'resize-pane', '-Z', '-t', panes[2]})
assert(last_offset == 1, 'unzooming the sibling must restore the inactive pane')
image_tmux.unwatch()
image_tmux.unwatch()
local previous = callbacks
tmux('select-window', '-t', 'image-watch:1')
tmux('set-option', '-g', 'status-position', 'bottom')
vim.wait(200)
assert(callbacks == previous, 'unwatched callback still ran')
local request = vim.uv.fs_scandir(vim.env.IPYNB_TEST_TMP)
if request then
  while true do
    local name = vim.uv.fs_scandir_next(request)
    if not name then break end
    assert(not name:match('^watch%-'), 'unwatch left a watcher file: ' .. name)
  end
end
"#,
    )
    .unwrap();

    let output = Command::new(std::env::var_os("IPYNB_TEST_NVIM").unwrap_or_else(|| "nvim".into()))
        .args(["--headless", "-u", "NONE", "-l"])
        .arg(&script)
        .env("IPYNB_TEST_ROOT", root)
        .env("IPYNB_TEST_SOCKET", &socket)
        .env("IPYNB_TEST_PANE_TTY", tty)
        .env("IPYNB_TEST_TMP", fixture.path())
        .env("TMUX", tmux_env)
        .env("TMUX_PANE", pane)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let hooks_after = String::from_utf8(tmux(&socket, &["show-hooks", "-g"]).stdout).unwrap();
    assert_eq!(hooks_after, hooks_before);
}

#[test]
#[ignore = "requires ImageMagick 7"]
fn plot_corners_preserve_aspect_ratio_inside_the_cell_canvas() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let base = root.join(".tmp/image-kitty");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::tempdir_in(base).unwrap();
    let source = fixture.path().join("source.png");
    let result = fixture.path().join("rounded.png");
    let script = fixture.path().join("round.lua");
    assert!(
        Command::new("magick")
            .args(["-size", "500x400", "xc:white"])
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    let original = fs::read(&source).unwrap();
    fs::write(
        &script,
        r#"
vim.opt.rtp:prepend(vim.env.IPYNB_TEST_ROOT)
local style = require('ipynb.image_style')
local radius, stroke, inner = style.metrics({cell_width=10, cell_height=22})
local doubled, doubled_stroke, doubled_inner = style.metrics({cell_width=20, cell_height=44})
assert(radius == 5 and stroke == 1 and inner == 1 and inner < radius)
assert(doubled == radius * 2 and doubled_stroke == stroke * 2 and doubled_inner == inner * 2,
  'corner radii must scale with terminal cell width')
local done = false
style.rounded(vim.env.IPYNB_TEST_SOURCE, 640, 506, inner, 500, 400, function(path)
  assert(vim.uv.fs_copyfile(path, vim.env.IPYNB_TEST_RESULT))
  done = true
end)
assert(vim.wait(10000, function() return done end), 'rounded image conversion did not finish')
"#,
    )
    .unwrap();
    let output = Command::new("nvim")
        .args(["--headless", "-u", "NONE", "-l"])
        .arg(&script)
        .env("IPYNB_TEST_ROOT", root)
        .env("IPYNB_TEST_SOURCE", &source)
        .env("IPYNB_TEST_RESULT", &result)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pixels = Command::new("magick")
        .arg(&result)
        .args(["-format", "%w %h %[fx:p{0,0}.a] %[fx:p{632,0}.a] %[fx:p{639,250}.a] %[fx:p{300,250}.a] %[fx:p{632,250}.a]", "info:"])
        .output()
        .unwrap();
    assert!(pixels.status.success());
    let pixels: Vec<f32> = String::from_utf8(pixels.stdout)
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse().unwrap())
        .collect();
    assert_eq!(
        &pixels[..2],
        &[640.0, 506.0],
        "canvas dimensions must stay exact"
    );
    assert!(
        pixels[2] > 0.0 && pixels[2] < 1.0,
        "rounded corner must be antialiased"
    );
    assert!(
        (pixels[2] - pixels[3]).abs() < 0.001,
        "top corners must be symmetric"
    );
    assert_eq!(pixels[4], 0.0, "canvas padding must remain transparent");
    assert_eq!(
        &pixels[5..],
        &[1.0, 1.0],
        "image content must remain opaque"
    );
    assert_eq!(
        fs::read(source).unwrap(),
        original,
        "display styling must not change notebook assets"
    );
}
