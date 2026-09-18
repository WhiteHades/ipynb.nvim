local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h")
vim.opt.rtp:prepend(root)
local sent
local helpers = { write_graphics_at = function(config, x, y) sent = {config, x, y} end }
package.loaded['image/backends/kitty/helpers'] = helpers
local bridge = require('ipynb.image_kitty')
local img = { internal_id = 7, global_state = { options = { backend = 'kitty' } } }
bridge.track(img, {get_size = function() return {cell_width=8,cell_height=16} end})
-- Docker's guessed pixels must not determine the displayed size. The terminal
-- expands this crop to the same cells reserved by the notebook output layout.
local config = {action='p',image_id=7,display_width=320,display_height=320,display_x=8,display_y=16}
helpers.write_graphics_at(config, 4, 6)
assert(sent[1].display_columns == 40 and sent[1].display_rows == 20)
assert(sent[1].display_x == 8 and sent[1].display_y == 16)
assert(sent[2] == 4 and sent[3] == 6)
assert(config.display_columns == nil) -- no mutation of provider-owned payload
config.image_id = 8
helpers.write_graphics_at(config, 1, 1)
assert(sent[1].display_columns == nil) -- other Markdown images retain their behavior
print('pass: notebook Kitty placements use cell bounds, including cropped images')
vim.cmd('qa!')
