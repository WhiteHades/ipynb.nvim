"""Output layout in real headless Neovim, with a deterministic image provider.

Run with PYTHONPATH=rplugin/python3 python -m unittest discover -s tests.
Set IPYNB_TEST_IMAGE_NVIM to an installed image.nvim directory to also exercise
its real processor and renderer, recording backend draws instead of emitting
terminal graphics. Headless Neovim has no pixel size, so tests supply cell pixels.
"""
import base64
from contextlib import contextmanager
import os
from pathlib import Path
import shutil
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

import pynvim
from PIL import Image

from ipynb_runtime import Ipynb
from ipynb_runtime.images import ImageNvimCanvas, NoCanvas
from ipynb_runtime.options import IpynbOptions
from ipynb_runtime.moltenbuffer import IpynbKernel
from ipynb_runtime.outputbuffer import OutputBuffer
from ipynb_runtime.outputchunks import ImageOutputChunk, OutputStatus, TextOutputChunk, to_outputchunk

ROOT = Path(__file__).resolve().parents[1]
(ROOT / ".tmp").mkdir(exist_ok=True)


@unittest.skipUnless(shutil.which("nvim"), "Neovim is required")
class ImageOutputTests(unittest.TestCase):
    real_provider = False

    def setUp(self):
        self.nvim = pynvim.attach("child", argv=["nvim", "--embed", "--headless", "-u", "NONE"])
        self.nvim.exec_lua("vim.opt.rtp:prepend(...)", str(ROOT))
        self.nvim.command("set columns=100 lines=50 laststatus=0 noruler noshowcmd number relativenumber list wrap")
        self.nvim.current.buffer[:] = ["cell"] * 10
        self.nvim.exec_lua("""
            draws, created, cleared = {}, {}, {}
            cell_pixels = { cell_width = 10, cell_height = 20, screen_cols = 100, screen_rows = 50 }
            package.loaded['image.utils.term'] = { get_size = function() return cell_pixels end }
            package.loaded['image/utils/term'] = package.loaded['image.utils.term']
        """)
        if self.real_provider:
            self.nvim.exec_lua("""
                vim.opt.rtp:prepend(...)
                package.loaded['image/backends/kitty'] = {
                  features = { crop = true }, setup = function() end,
                  render = function(img, x, y, width, height)
                    img.is_rendered = true
                    img.global_state.images[img.id] = img
                    draws[img.id] = { x=x, y=y, width=width, height=height, window=img.window }
                  end,
                  clear = function(id) if id then draws[id] = nil; cleared[id] = true end end,
                }
                require('image').setup({ processor = 'magick_cli', backend = 'kitty',
                  max_height_window_percentage = 100, max_width_window_percentage = 100,
                  integrations = {}, hijack_file_patterns = {} })
            """, os.environ["IPYNB_TEST_IMAGE_NVIM"])
        else:
            self.nvim.exec_lua("""
                package.loaded.image = { from_file = function(path, opts)
                  if path == 'bad' then return nil end
                  local dimensions = { wide={1200,600}, tall={300,1800}, small={20,10} }
                  local size = dimensions[path] or dimensions.wide
                  local img = { image_width=size[1], image_height=size[2], geometry={},
                    global_state={options={max_height_window_percentage=100}} }
                  function img:render()
                    draws[opts.id] = { width=self.geometry.width, height=self.geometry.height,
                      y=self.geometry.y, offset=self.render_offset_top, window=self.window }
                  end
                  function img:clear() draws[opts.id]=nil; cleared[opts.id]=true end
                  created[opts.id] = img
                  return img
                end }
            """)
        self.canvas = ImageNvimCanvas(self.nvim)
        self.canvas.init()
        self.options = IpynbOptions(self.nvim)
        self.options.image_provider = "image.nvim"
        self.options.wrap_output = True
        self.options.output_win_border = "single"
        self.output = OutputBuffer(self.nvim, self.canvas, self.nvim.api.create_namespace("test"), self.options)
        self.output.output.status = OutputStatus.DONE
        self.anchor = SimpleNamespace(bufno=self.nvim.current.buffer.number, lineno=1)

    def tearDown(self):
        try:
            self.canvas.deinit()
            self.nvim.command("qa!")
        except EOFError:
            pass
        finally:
            self.nvim.close()

    def virtual_lines(self):
        return self.nvim.api.buf_get_extmark_by_id(
            self.anchor.bufno, self.output.extmark_namespace, self.output.virt_text_id, {"details": True}
        )[2]["virt_lines"]

    def test_small_plot_enlarges_to_bounded_inline_area(self):
        chunk = ImageOutputChunk("small")
        self.output.output.chunks = [chunk]
        self.output.show_virtual_output(self.anchor)
        size = self.canvas.img_size(chunk.img_identifier)
        self.assertGreater(size["width"], 40)
        self.assertLessEqual(size["width"], int(self.nvim.current.window.width * .9))
        self.assertLessEqual(size["height"], int(self.nvim.current.window.height * .75))

    def test_inline_height_is_not_limited_by_markdown_percentage(self):
        chunk = ImageOutputChunk("tall")
        self.output.output.chunks = [chunk]
        self.output.show_virtual_output(self.anchor)
        self.nvim.exec_lua("created[...].global_state.options.max_height_window_percentage=50", chunk.img_identifier)
        self.output._virtual_shape = None
        self.output.show_virtual_output(self.anchor)
        size = self.canvas.img_size(chunk.img_identifier)
        self.assertEqual(size["height"], int((self.nvim.current.window.height - 2) * .75))

    def test_popup_command_opens_first_image_without_external_viewer(self):
        kernel = object.__new__(IpynbKernel)
        kernel.nvim = self.nvim
        kernel.options = self.options
        cell = object()
        kernel._get_selected_span = lambda: cell
        self.output.output.chunks = [TextOutputChunk("before"), ImageOutputChunk("first"),
                                     ImageOutputChunk("second")]
        kernel.outputs = {cell: self.output}
        self.nvim.exec_lua("package.loaded['ipynb.image_viewer'] = {open=function(path) opened=path end}")
        self.assertTrue(kernel.open_image_popup())
        self.assertEqual(self.nvim.exec_lua("return opened"), "first")

    def test_click_uses_rendered_rectangle_and_leaves_text_clicks_alone(self):
        chunk = ImageOutputChunk("small")
        self.output.output.chunks = [chunk]
        self.output.show_virtual_output(self.anchor)
        result = self.nvim.exec_lua("""
            local img = created[...]
            img.is_rendered = true
            img.rendered_geometry = {x=5, y=10, width=40, height=12}
            package.loaded['ipynb.image_viewer'] = {open=function(path) opened=path end}
            local callback
            for _, map in ipairs(vim.api.nvim_buf_get_keymap(0, 'n')) do
              if map.lhs == '<LeftMouse>' then callback = map.callback end
            end
            local pos = {winid=img.window, screenrow=12, screencol=7}
            vim.fn.getmousepos = function() return pos end
            local hit = callback()
            pos.screenrow = 10 -- immediately above the image
            local miss = callback()
            img.is_rendered = false
            pos.screenrow = 12
            local hidden = callback()
            return {hit, miss, hidden}
        """, chunk.img_identifier)
        self.assertEqual(result, ["<Ignore>", "<LeftMouse>", "<LeftMouse>"])
        self.assertEqual(self.nvim.exec_lua("return opened"), "small")

    def test_inline_image_rows_are_not_text_preview_lines(self):
        image = ImageOutputChunk("wide")
        self.output.output.chunks = [image]
        self.output.show_virtual_output(self.anchor)
        lines = self.virtual_lines()
        self.assertGreater(len(lines), self.options.virt_text_max_lines)
        self.assertFalse(any("More lines" in str(line) for line in lines))
        self.assertEqual(len(lines), self.canvas.img_size(image.img_identifier)["height"] + 2)

    def test_provider_independent_preview_and_bottom_float_regression(self):
        # This provider contract also works with the old layout, isolating the
        # two original bugs from changes to the Lua bridge API.
        canvas = Mock()
        canvas.add_image.side_effect = lambda path, identifier, *args, **kwargs: identifier
        canvas.img_size.return_value = {"width": 96, "height": 24}
        canvas.layout_key.return_value = None
        self.output.canvas = canvas
        self.output.output.chunks = [ImageOutputChunk("plot.png")]
        self.output.show_virtual_output(self.anchor)
        with self.subTest("inline image must not be truncated by text preview"):
            self.assertEqual(len(self.virtual_lines()), 26)
        self.output._buffer_to_window_lineno = lambda _: self.nvim.current.window.height - 3
        self.output.show_floating_win(self.anchor)
        with self.subTest("float must use available window space above the anchor"):
            self.assertEqual(self.output.display_win.height, 26)
        with self.subTest("image rows must be scrollable real buffer lines"):
            self.assertEqual(len(self.output.display_buf), 26)

    def test_float_near_bottom_has_full_image_rows_and_clears_inline(self):
        image = ImageOutputChunk("wide")
        self.output.output.chunks = [image]
        self.output.show_virtual_output(self.anchor)
        inline_id = image.img_identifier
        self.output._buffer_to_window_lineno = lambda _: self.nvim.current.window.height - 2
        self.output.show_floating_win(self.anchor)
        floating_id = image.img_identifier
        self.assertEqual(self.nvim.funcs.getwininfo(self.output.display_win.handle)[0]["textoff"], 0)
        for option in ("number", "relativenumber", "list", "wrap"):
            self.assertFalse(self.output.display_win.options[option], option)
        self.assertNotEqual(inline_id, floating_id)
        height = self.canvas.img_size(floating_id)["height"]
        self.assertEqual(self.output.display_win.height, height + 2)
        self.assertEqual(len(self.output.display_buf), height + 2)
        draws = self.nvim.exec_lua("return draws")
        self.assertNotIn(inline_id, draws)
        self.assertEqual(draws[floating_id]["window"], self.output.display_win.handle)
        self.output.clear_float_win()
        self.output.show_virtual_output(self.anchor)
        self.assertIn(inline_id, self.nvim.exec_lua("return draws"))

    def test_mixed_text_multiple_images_have_nonoverlapping_rows(self):
        first, second = ImageOutputChunk("wide"), ImageOutputChunk("wide")
        self.output.output.chunks = [TextOutputChunk("before\n"), first,
                                     TextOutputChunk("between\n"), second, TextOutputChunk("after\n")]
        self.output.show_virtual_output(self.anchor)
        draws = self.nvim.exec_lua("return draws")
        a, b = draws[first.img_identifier], draws[second.img_identifier]
        self.assertEqual(a["offset"], 2)
        self.assertEqual(b["offset"], 3 + self.canvas.img_size(first.img_identifier)["height"])
        lines = [row[0][0].rstrip() for row in self.virtual_lines()]
        self.assertEqual(lines[1], "before")
        self.assertEqual(lines[b["offset"] - 1], "between")
        self.assertEqual(lines[-2], "after")

    def test_done_output_reflows_when_window_resizes(self):
        image = ImageOutputChunk("wide")
        self.output.output.chunks = [image]
        self.output.show_virtual_output(self.anchor)
        before = len(self.virtual_lines())
        self.nvim.command("vsplit")
        self.nvim.current.window.width = 24
        self.output.show_virtual_output(self.anchor)
        after = len(self.virtual_lines())
        self.assertLess(after, before)
        size = self.canvas.img_size(image.img_identifier)
        self.assertLessEqual(size["width"], 24)
        self.assertEqual(after, size["height"] + 2)
        self.nvim.exec_lua("cell_pixels.cell_width=5; cell_pixels.cell_height=20")
        self.output.show_virtual_output(self.anchor)
        self.assertLess(len(self.virtual_lines()), after)

    def test_tall_wide_small_and_changed_cell_pixels_preserve_aspect(self):
        for path, ratio in [("wide", 2), ("tall", 1/6), ("small", 2)]:
            for width, height in [(1, 4), (20, 30), (100, 8)]:
                with self.subTest(path=path, width=width, height=height):
                    identifier = self.canvas.add_image(path, path, 0, 0, self.anchor.bufno,
                                                      max_width=width, max_height=height)
                    size = self.canvas.img_size(identifier)
                    self.assertTrue(0 < size["width"] <= width)
                    self.assertTrue(0 < size["height"] <= height)
                    geometry = self.nvim.exec_lua("return created[...].geometry", identifier)
                    self.assertAlmostEqual(geometry["width"] * 10 / (geometry["height"] * 20), ratio)
        self.nvim.exec_lua("cell_pixels.cell_width=20; cell_pixels.cell_height=40")
        size = self.canvas.img_size("small")
        self.assertEqual(size, {"width": 32, "height": 8})

    def test_terminal_response_reflows_without_mutating_provider_cache(self):
        image = ImageOutputChunk("wide")
        self.output.output.chunks = [image]
        self.output.show_virtual_output(self.anchor)
        original_rows = len(self.virtual_lines())
        for height, width in [(20, 9), (30, 13)]:
            self.nvim.api.exec_autocmds("TermResponse", {"data": {"sequence": f"\x1b[6;{height};{width}t"}})
            self.assertEqual(self.canvas.layout_key(), [width, height])
            self.assertEqual(self.nvim.exec_lua("return {cell_pixels.cell_width,cell_pixels.cell_height}"), [10, 20])
            self.output.show_virtual_output(self.anchor)
            geometry = self.nvim.exec_lua("return draws[... ]", image.img_identifier)
            self.assertAlmostEqual(geometry["width"] * width / (geometry["height"] * height), 2)
            self.assertLess(len(self.virtual_lines()), original_rows)
        for invalid in ["\x1b[?62;52c", "\x1b[6;0;9t", "\x1b[6;20;0t", "\x1b[6;-1;9t", "\x1b[6;20;9"]:
            self.nvim.api.exec_autocmds("TermResponse", {"data": {"sequence": invalid}})
            self.assertEqual(self.canvas.layout_key(), [13, 30])
        self.nvim.command("set columns=77 lines=31")
        size = self.nvim.exec_lua("return require('image.utils.term').get_size()")
        self.assertEqual((size["screen_cols"], size["screen_rows"]), (77, 31))
        self.assertEqual((size["screen_x"], size["screen_y"]), (77 * 13, 31 * 30))
        self.assertEqual(self.nvim.exec_lua("return cell_pixels.screen_cols"), 100)

    def test_terminal_pixel_queries_follow_resize_and_focus(self):
        self.nvim.exec_lua("""
            pixel_queries = {}
            vim.api.nvim_list_uis = function() return {{chan=1}} end
            vim.api.nvim_ui_send = function(sequence) pixel_queries[#pixel_queries+1] = sequence end
        """)
        for event in ["VimResized", "FocusGained"]:
            self.nvim.api.exec_autocmds(event, {})
        self.assertEqual(self.nvim.exec_lua("return pixel_queries"), ["\x1b[16t", "\x1b[16t"])

    def test_text_preview_limit_does_not_consume_or_clip_images(self):
        self.output.output.chunks = [TextOutputChunk("line\n" * 40), ImageOutputChunk("wide")]
        self.output.show_virtual_output(self.anchor)
        lines = self.virtual_lines()
        self.assertTrue(any("More lines" in str(line) for line in lines))
        image_height = self.canvas.img_size(self.output.output.chunks[-1].img_identifier)["height"]
        self.assertEqual(len(lines), self.options.virt_text_max_lines + image_height)

    def test_exact_width_text_does_not_add_phantom_image_offset(self):
        self.options.wrap_output = True
        self.options.output_win_max_width = 20
        image = ImageOutputChunk("wide")
        self.output.output.chunks = [TextOutputChunk("x" * 20 + "\n"), image]
        self.output.show_floating_win(self.anchor)
        self.assertEqual(len(self.output.display_buf), 8)
        self.assertEqual(self.output.display_buf[1], "x" * 20)
        self.assertEqual(self.nvim.exec_lua("return draws[...].y", image.img_identifier), 1)

    def test_empty_output_and_failed_or_disabled_provider_have_visible_fallback(self):
        self.output.show_floating_win(self.anchor)
        self.assertEqual(self.output.display_win.height, 2)
        self.output.clear_float_win()
        for canvas, path in [(NoCanvas(), "wide"), (self.canvas, "bad")]:
            image = ImageOutputChunk(path)
            image.jupyter_data = {"text/plain": "fallback representation"}
            self.output.canvas = canvas
            self.output.output.chunks = [image]
            lines, _ = self.output.build_output_text((0, 0, 80, 30), self.anchor.bufno, False)
            self.assertIn("fallback representation", "\n".join(lines))

    def test_float_width_limit_applies_before_image_measurement(self):
        self.options.output_win_max_width = 20
        image = ImageOutputChunk("wide")
        self.output.output.chunks = [image]
        self.output.show_floating_win(self.anchor)
        self.assertEqual(self.output.display_win.width, 20)
        self.assertEqual(self.canvas.img_size(image.img_identifier), {"width": 20, "height": 5})
        self.assertEqual(self.output.display_win.height, 7)

    def test_enter_output_with_offscreen_cell_end_and_closed_previous_window(self):
        self.options.enter_output_behavior = "open_and_enter"
        self.output.output.chunks = [ImageOutputChunk("tall")]
        self.output._buffer_to_window_lineno = lambda _: -1
        source = self.nvim.current.window
        self.output.enter(self.anchor)
        self.assertEqual(self.nvim.current.window, self.output.display_win)
        self.assertGreater(self.output.display_win.height, 1)
        self.nvim.api.win_close(self.output.display_win, True)
        self.assertEqual(self.nvim.current.window, source)
        self.output.enter(self.anchor)
        self.assertEqual(self.nvim.current.window, self.output.display_win)

    def test_provider_limits_apply_once_and_missing_terminal_geometry_falls_back(self):
        image = ImageOutputChunk("wide")
        self.output.output.chunks = [image]
        self.output.show_virtual_output(self.anchor)
        self.nvim.exec_lua("created[...].global_state.options.max_height_window_percentage=50", image.img_identifier)
        self.output._virtual_shape = None
        self.output.show_virtual_output(self.anchor)
        size = self.canvas.img_size(image.img_identifier)
        self.assertLessEqual(size["height"], (self.nvim.current.window.height - 2) // 2)
        self.nvim.exec_lua("cell_pixels.cell_width=0")
        self.output.show_virtual_output(self.anchor)
        self.assertIn("Image unavailable", str(self.virtual_lines()))
        self.assertNotIn(image.img_identifier, self.nvim.exec_lua("return draws"))

    def test_float_scroll_moves_images_with_their_buffer_rows(self):
        first, second = ImageOutputChunk("wide"), ImageOutputChunk("tall")
        self.output.output.chunks = [first, TextOutputChunk("middle\n"), second]
        self.output.show_floating_win(self.anchor)
        self.nvim.current.window = self.output.display_win
        before = self.nvim.exec_lua("return draws[...].y", second.img_identifier)
        self.nvim.command("normal! Gzt")
        self.nvim.command("doautocmd WinScrolled")
        top = self.nvim.funcs.getwininfo(self.output.display_win.handle)[0]["topline"]
        after = self.nvim.exec_lua("return draws[...].y", second.img_identifier)
        self.assertGreater(top, 1)
        self.assertEqual(after, before - top + 1)

    def test_float_masks_other_cells_even_during_provider_redraws(self):
        background = ImageOutputChunk("wide")
        self.output.output.chunks = [background]
        self.output.show_virtual_output(self.anchor)
        identifier = background.img_identifier
        other = OutputBuffer(self.nvim, self.canvas, self.output.extmark_namespace, self.options)
        other.output.chunks = [ImageOutputChunk("wide")]
        other.show_floating_win(self.anchor)
        self.assertNotIn(identifier, self.nvim.exec_lua("return draws"))
        # image.nvim can call the image directly when an async resize completes.
        self.nvim.exec_lua("created[...]:render()", identifier)
        self.assertNotIn(identifier, self.nvim.exec_lua("return draws"))
        other.clear_float_win()
        self.assertIn(identifier, self.nvim.exec_lua("return draws"))

    def test_focused_float_resize_callback_uses_source_window_and_restores_focus(self):
        self.output.output.chunks = [ImageOutputChunk("wide")]
        self.output.show_floating_win(self.anchor)
        source = self.nvim.current.window
        floating = self.output.display_win
        self.nvim.current.window = floating
        runtime = object.__new__(Ipynb)
        runtime.initialized = True
        runtime.nvim = self.nvim
        runtime._get_current_buf_kernels = lambda _: None
        kernel = SimpleNamespace(outputs={Mock(end=self.anchor): self.output})
        kernel.update_interface = Mock(side_effect=lambda: self.output.show_floating_win(self.anchor))
        runtime.ipynb_kernels = {"python": kernel}
        self.nvim.command("set columns=40")
        # A resize may move the hidden source cursor out of the selected cell.
        source.cursor = (10, 0)
        runtime._update_interface()
        self.assertEqual(self.nvim.current.window, floating)
        self.assertEqual(self.output.source_window, source.handle)
        self.assertLessEqual(floating.width, 38)
        self.assertGreater(floating.height, 1)
        self.assertEqual(source.cursor, self.output.source_cursor)
        kernel.update_interface.assert_not_called()


class MimeTests(unittest.TestCase):
    def test_png_jpeg_svg_lists_and_no_provider_fallback(self):
        with TemporaryDirectory(dir=ROOT / ".tmp") as directory:
            @contextmanager
            def alloc(extension, mode):
                path = Path(directory) / ("output." + extension)
                with path.open(mode) as handle:
                    yield str(path), handle

            for extension in ["png", "jpeg"]:
                path = Path(directory) / ("source." + extension)
                Image.new("RGB", (600, 300)).save(path)
                encoded = base64.b64encode(path.read_bytes()).decode()
                for data in [encoded, [encoded[:20], encoded[20:]]]:
                    chunk = to_outputchunk(None, alloc, {"image/" + extension: data}, {},
                                           SimpleNamespace(image_provider="image.nvim"))
                    self.assertIsInstance(chunk, ImageOutputChunk)
                    with Image.open(chunk.img_path) as decoded:
                        self.assertEqual(decoded.size, (600, 300))
            chunk = to_outputchunk(None, alloc, {"image/svg+xml": [
                '<svg xmlns="http://www.w3.org/2000/svg" width="600" height="300">',
                '<rect width="600" height="300" fill="red"/></svg>']}, {},
                SimpleNamespace(image_provider="image.nvim"))
            self.assertIsInstance(chunk, ImageOutputChunk)
            chunk = to_outputchunk(None, alloc, {"image/png": "invalid", "text/plain": ["plain"]}, {},
                                   SimpleNamespace(image_provider="none"))
            self.assertEqual(chunk.text, "plain\n")
            chunk = to_outputchunk(None, alloc, {"image/png": "invalid", "text/plain": "plain"}, {},
                                   SimpleNamespace(image_provider="image.nvim"))
            self.assertEqual(chunk.text, "plain\n")


@unittest.skipUnless(os.environ.get("IPYNB_TEST_IMAGE_NVIM") and shutil.which("magick"),
                     "set IPYNB_TEST_IMAGE_NVIM and install ImageMagick for real renderer tests")
class RealImageRendererTests(unittest.TestCase):
    real_provider = True
    setUp = ImageOutputTests.setUp
    tearDown = ImageOutputTests.tearDown

    def test_real_png_jpeg_svg_render_geometry(self):
        with TemporaryDirectory(dir=ROOT / ".tmp") as directory:
            for extension in ["png", "jpeg", "svg"]:
                path = Path(directory) / ("plot." + extension)
                if extension == "svg":
                    path.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="600">'
                                    '<rect width="1200" height="600" fill="red"/></svg>')
                else:
                    Image.new("RGB", (1200, 600), "red").save(path)
                chunk = ImageOutputChunk(str(path))
                self.output.output.chunks = [chunk]
                self.output.show_floating_win(self.anchor)
                rendered = self.nvim.exec_lua("local id=...; return vim.wait(5000, function() return draws[id] ~= nil end)",
                                              chunk.img_identifier)
                self.assertTrue(rendered, (extension, chunk.img_identifier,
                                          self.nvim.command_output('messages'),
                                          self.nvim.exec_lua("return vim.inspect(require('image').get_images())")))
                draw = self.nvim.exec_lua("return draws[...]", chunk.img_identifier)
                origin = self.nvim.funcs.screenpos(self.output.display_win.handle, 1, 1)
                self.assertEqual(draw["y"], origin["row"])
                self.assertEqual(draw["x"], origin["col"] - 1)
                self.assertGreater(draw["height"], 1)
                self.assertLessEqual(draw["height"], self.output.display_win.height - 2)
                self.assertLessEqual(draw["width"], self.output.display_win.width)
                self.assertAlmostEqual(draw["width"] * 10 / (draw["height"] * 20), 2, delta=.1)
                self.output.clear_float_win()

    def test_real_viewer_enlarges_and_restores_inline_image(self):
        with TemporaryDirectory(dir=ROOT / ".tmp") as directory:
            path = Path(directory) / "small.png"
            Image.new("RGB", (100, 100), "red").save(path)
            chunk = ImageOutputChunk(str(path))
            self.output.output.chunks = [chunk]
            self.output.show_virtual_output(self.anchor)
            self.assertTrue(self.nvim.exec_lua(
                "local id=...; return vim.wait(5000, function() return draws[id] ~= nil end)", chunk.img_identifier))
            source = self.nvim.current.window
            self.assertTrue(self.nvim.exec_lua("return require('ipynb.image_viewer').open(...)", str(path)))
            viewer = self.nvim.current.window
            self.assertNotEqual(source, viewer)
            self.assertTrue(self.nvim.exec_lua("""
                return vim.wait(5000, function()
                  for _, draw in pairs(draws) do
                    if draw.window == vim.api.nvim_get_current_win() then return true end
                  end
                end)
            """))
            self.assertNotIn(chunk.img_identifier, self.nvim.exec_lua("return draws"))
            self.assertGreater(viewer.height, 20)
            self.assertLessEqual(viewer.width + 2, int(self.nvim.options['columns'] * .92))
            self.assertLessEqual(viewer.height + 2, int(self.nvim.options['lines'] * .92))
            self.nvim.exec_lua("require('ipynb.image_viewer').close()")
            self.assertEqual(self.nvim.current.window, source)
            self.assertTrue(self.nvim.exec_lua(
                "local id=...; return vim.wait(5000, function() return draws[id] ~= nil end)", chunk.img_identifier))

    def test_real_float_scroll_repositions_second_image(self):
        with TemporaryDirectory(dir=ROOT / ".tmp") as directory:
            path = Path(directory) / "plot.png"
            Image.new("RGB", (800, 400), "red").save(path)
            first, second = ImageOutputChunk(str(path)), ImageOutputChunk(str(path))
            self.options.output_win_max_height = 20
            self.output.output.chunks = [TextOutputChunk("before\n"), first,
                                         TextOutputChunk("between\n"), second,
                                         TextOutputChunk("after\n")]
            self.output.show_floating_win(self.anchor)
            self.nvim.exec_lua("vim.wait(1000, function() return next(draws) ~= nil end)")
            self.nvim.current.window = self.output.display_win
            self.nvim.command("normal! Gzb")
            self.nvim.command("doautocmd WinScrolled")
            self.assertTrue(self.nvim.exec_lua(
                "local id=...; return vim.wait(5000, function() return draws[id] ~= nil end)", second.img_identifier
            ))
            draw = self.nvim.exec_lua("return draws[...]", second.img_identifier)
            self.assertEqual(draw["window"], self.output.display_win.handle)
            self.assertGreater(self.nvim.funcs.getwininfo(self.output.display_win.handle)[0]["topline"], 1)
            top = self.nvim.funcs.getwininfo(self.output.display_win.handle)[0]["topline"]
            origin = self.nvim.funcs.screenpos(self.output.display_win.handle, top, 1)
            self.assertGreaterEqual(origin["row"], 1)
            self.assertEqual(draw["x"], origin["col"] - 1)


if __name__ == "__main__":
    unittest.main()
