use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

#[test]
fn notebook_kitty_placements_preserve_geometry_and_finish_the_frame() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let base = root.join(".tmp/image-kitty");
    fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::tempdir_in(base).unwrap();
    let script = fixture.path().join("check.lua");
    let report = fixture.path().join("report.json");
    fs::write(&script, r#"
vim.opt.rtp:prepend(vim.env.IPYNB_TEST_ROOT)
local writes, deferred, counts, loads, finishes = {}, {}, {}, 0, 0
local helpers = {
  write_graphics_at = function(config, x, y)
    writes[#writes + 1] = {config=vim.deepcopy(config), x=x, y=y}
    return 'sent'
  end,
  update_sync_end = function() finishes = finishes + 1 end,
}

package.preload['image/backends/kitty/helpers'] = function() loads = loads + 1; return helpers end
vim.defer_fn = function(callback, delay) deferred[#deferred + 1] = {callback=callback, delay=delay} end
local cold = package.loaded['image/backends/kitty/helpers'] == nil
local bridge = require('ipynb.image_kitty')
local img = {internal_id=7, global_state={options={backend='kitty'}}}
bridge.track(img, {get_size=function() return {cell_width=8, cell_height=16} end})
bridge.track(img, {get_size=function() return {cell_width=8, cell_height=16} end})
local config = {action='p', image_id=7, display_width=320, display_height=320, display_x=8, display_y=16}
local result = helpers.write_graphics_at(config, 4, 6)
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
package.loaded['ipynb.image_kitty'] = nil
package.loaded['image/backends/kitty/helpers'] = {}
local warnings = {}
vim.notify = function(message) warnings[#warnings + 1] = message end
local unsupported = require('ipynb.image_kitty')
local survived, failure = pcall(function() unsupported.track(img, {}); unsupported.track(img, {}) end)
vim.fn.writefile({vim.json.encode({cold=cold, loads=loads, writes=writes, original=config,
  other=other, result=result, counts=counts, delays=delays, before=before, finishes=finishes,
  survived=survived, failure=failure, warnings=warnings})}, vim.env.IPYNB_TEST_REPORT)
-- Keep tmux variants in this transport smoke check, not a separate Lua runner.
package.loaded['ipynb.image_kitty'], package.loaded['ipynb.image_tmux'] = nil, nil
local tmux, queries, scheduled = {is_tmux=true}, 0, {}
local reply = '1 1 1 top on 40 0\n'
package.preload['image/utils/tmux'] = function() return tmux end
package.loaded['image/utils/tmux'] = nil
local last, count = nil, 0
helpers = {update_sync_end=function() end, write_graphics_at=function(value, x, y)
  last = {config=value, x=x + (tmux.is_tmux and 12 or 0), y=y + (tmux.is_tmux and 7 or 0)}
  count = count + 1
  return 'sent'
end}
package.loaded['image/backends/kitty/helpers'] = helpers
vim.env.TMUX_PANE = '%42'
vim.fn.system = function(command)
  assert(command[1] == 'tmux' and command[4] == '-t' and command[5] == '%42')
  queries = queries + 1
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
assert(queries == 1, 'one tmux snapshot per render batch')
for _, case in ipairs({{'bottom on 40',13},{'top off 40',13},{'top 2 40',15},
  {'top 0 40',13},{'top 1 40',14},{'bottom 2 40',13},{'top 2 2',13},{'top 2 1',13}}) do
  batch('1 1 1 ' .. case[1] .. ' 0\n'); place(case[2])
end
for _, hidden in ipairs({'0 1 1 top on 40 0','1 0 1 top on 40 0',
  '1 1 0 top on 40 0','1 1 1 top on 40 1','malformed','1 1 1 top 9 40 0'}) do
  batch(hidden); place(nil)
end
batch('1 1 1 top 2 40 0'); place(15)
img.global_state.disable_decorator_handling = true; place(nil)
img.global_state.disable_decorator_handling = false
batch(); vim.env.TMUX_PANE = nil
local before_queries = queries
place(nil); assert(queries == before_queries)
batch(); vim.env.TMUX_PANE = '%42'; tmux.is_tmux = false
helpers.write_graphics_at(config, 4, 6)
assert(last.x == 4 and last.y == 6 and queries == before_queries)
tmux.is_tmux = true
helpers.write_graphics_at(other, 4, 6)
assert(last.x == 16 and last.y == 13 and queries == before_queries)
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
    assert_eq!(observed["result"], "sent");
    assert_eq!(observed["writes"].as_array().unwrap().len(), 4);
    let original = json!({"action":"p", "image_id":7, "display_width":320,
        "display_height":320, "display_x":8, "display_y":16});
    assert_eq!(observed["original"], original);
    let mut other = original.clone();
    other["image_id"] = json!(8);
    assert_eq!(observed["other"], other);
    let mut placed = original;
    placed["display_columns"] = json!(40);
    placed["display_rows"] = json!(20);
    assert_eq!(
        observed["writes"][0],
        json!({"config":placed, "x":4, "y":6})
    );
    assert_eq!(
        observed["writes"][1],
        json!({"config":observed["other"], "x":1, "y":2})
    );
    assert_eq!(observed["other"]["display_columns"], Value::Null);
    assert_eq!(observed["counts"], json!([1, 1, 1, 2]));
    assert_eq!(observed["delays"], json!([20, 20]));
    assert_eq!(observed["before"], 0);
    assert_eq!(observed["finishes"], 2);
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
assert(math.abs(radius - 15.4) < 0.001 and stroke == 2 and inner == radius * 0.75)
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
    assert_eq!(
        String::from_utf8(pixels.stdout).unwrap(),
        "640 506 0 0 0 1 1"
    );
    assert_eq!(
        fs::read(source).unwrap(),
        original,
        "display styling must not change notebook assets"
    );
}
