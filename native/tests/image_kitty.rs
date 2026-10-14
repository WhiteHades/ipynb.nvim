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
