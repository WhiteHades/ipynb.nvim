import re
from datetime import datetime
from typing import Any, Callable, List, Optional, Tuple, Union

from pynvim import Nvim
from pynvim.api import Buffer, Window

from ipynb_runtime.images import Canvas
from ipynb_runtime.options import IpynbOptions
from ipynb_runtime.outputchunks import ImageOutputChunk, Output, OutputStatus, TextOutputChunk
from ipynb_runtime.position import DynamicPosition, Position
from ipynb_runtime.utils import notify_error


def render_control_chars(text: str) -> str:
    """Render carriage returns and backspaces as terminal-style text updates."""
    if "\b" not in text and "\r" not in text:
        return text

    # Text chunks get a trailing newline for display. Drop that artificial
    # boundary when a saved progress update continues with backspaces.
    text = re.sub(r"\n(?=\x08+\r)", "", text)

    lines = [[]]
    column = 0
    for char in text:
        if char == "\n":
            lines.append([])
            column = 0
        elif char == "\r":
            column = 0
        elif char == "\b":
            column = max(0, column - 1)
        else:
            line = lines[-1]
            if column < len(line):
                line[column] = char
            else:
                line.append(char)
            column += 1

    return "\n".join("".join(line) for line in lines)


def truncate_bottom(lines: list[str], text_max_lines: int) -> list[str]:
    text_max_lines = max(1, text_max_lines)
    truncated_lines = lines[: text_max_lines - 1]
    truncated_lines.append(f"󰁅 {len(lines) - text_max_lines + 1} More lines ")
    return truncated_lines


def truncate_top(lines: list[str], text_max_lines: int):
    if text_max_lines <= 2:
        return truncate_bottom(lines, text_max_lines)
    truncated_lines = [lines[0]]
    truncated_lines.append(f"↑ {len(lines) - text_max_lines} More lines")
    truncated_lines.extend(lines[-text_max_lines + 2 :])
    return truncated_lines


class OutputBuffer:
    nvim: Nvim
    canvas: Canvas

    output: Output

    display_buf: Buffer
    display_win: Optional[Window]
    display_virt_lines: Optional[DynamicPosition]
    extmark_namespace: int
    virt_text_id: Optional[int]
    displayed_status: OutputStatus

    options: IpynbOptions
    lua: Any

    def __init__(self, nvim: Nvim, canvas: Canvas, extmark_namespace: int, options: IpynbOptions):
        self.nvim = nvim
        self.canvas = canvas

        self.output = Output(None)

        self.display_buf = self.nvim.buffers[self.nvim.funcs.nvim_create_buf(False, True)]
        self.display_win: Window | None = None
        self.display_virt_lines = None
        self.virt_hidden: bool = False
        self.extmark_namespace = extmark_namespace
        self.virt_text_id = None
        self.displayed_status = OutputStatus.HOLD
        self._virtual_shape = None
        self._float_shape = None
        self.source_window = None
        self.source_cursor = None

        self.options = options
        self.nvim.exec_lua("_ipynb_ow = require('ipynb.molten_output_window')")
        self.lua = self.nvim.lua._ipynb_ow

        self.truncate_lines: Callable[[list[str], int], list[str]]
        if self.options.virt_text_truncate == "bottom":
            self.truncate_lines = truncate_bottom
        elif self.options.virt_text_truncate == "top":
            self.truncate_lines = truncate_top
        else:
            raise ValueError("Wrong virtual text truncate option")

    def _buffer_to_window_lineno(self, lineno: int) -> int:
        return self.lua.calculate_window_position(lineno)

    def _get_header_text(self, output: Output) -> str:
        if output.execution_count is None:
            execution_count = "..."
        else:
            execution_count = str(output.execution_count)

        match output.status:
            case OutputStatus.HOLD:
                status = "* On Hold"
            case OutputStatus.DONE:
                if output.success:
                    status = "✓ Done"
                else:
                    status = "✗ Failed"
            case OutputStatus.RUNNING:
                status = "... Running"
            case OutputStatus.NEW:
                status = ""
            case _:
                raise ValueError("bad output.status: %s" % output.status)

        if output.old:
            old = "[OLD] "
        else:
            old = ""

        if not output.old and self.options.output_show_exec_time and output.start_time:
            start = output.start_time
            end = output.end_time if output.end_time is not None else datetime.now()
            diff = end - start

            days = diff.days
            hours = diff.seconds // 3600
            minutes = diff.seconds // 60
            seconds = diff.seconds - hours * 3600 - minutes * 60
            microseconds = diff.microseconds

            time = ""

            # Days
            if days:
                time += f"{days}d "
            if hours:
                time += f"{hours}hr "
            if minutes:
                time += f"{minutes}m "

            # Microseconds is an int, roundabout way to round to 2 digits
            time += f"{seconds}.{int(round(microseconds, -4) / 10000)}s"
        else:
            time = ""

        if output.status == OutputStatus.NEW:
            return f"╭─ Out[_]: Never Run"
        else:
            return f"╭─ {old}Out[{execution_count}]: {status} {time}".rstrip()

    def enter(self, anchor: Position) -> bool:
        entered = False
        if self.display_win is None or not self.display_win.valid:
            if self.options.enter_output_behavior == "open_then_enter":
                self.show_floating_win(anchor)
            elif self.options.enter_output_behavior == "open_and_enter":
                self.show_floating_win(anchor)
                if self.display_win is not None and self.display_win.valid:
                    entered = True
                    self.nvim.funcs.nvim_set_current_win(self.display_win)
        elif self.options.enter_output_behavior != "no_open":
            entered = True
            self.nvim.funcs.nvim_set_current_win(self.display_win)
        if entered:
            if self.options.output_show_more:
                self.remove_window_footer()
            if self.options.output_win_hide_on_leave:
                return False
        return True

    def clear_float_win(self) -> None:
        if self.display_win is not None:
            if self.display_win.valid:
                self.nvim.funcs.nvim_win_close(self.display_win, True)
            self.display_win = None
            self.clear_images(False)
            self._virtual_shape = None
        if self.display_virt_lines is not None:
            del self.display_virt_lines
            self.display_virt_lines = None

    def clear_virt_output(self, bufnr: int) -> None:
        if self.virt_text_id is not None:
            # remove the extmark…
            self.nvim.funcs.nvim_buf_del_extmark(bufnr, self.extmark_namespace, self.virt_text_id)
            # …and clear our flag so show_virtual_output can re-add it
            self.virt_text_id = None
            # (optional) reset displayed_status so your guard won’t block:
            # self.displayed_status = OutputStatus.NEW
            self.virt_hidden = True

        self.clear_images(True)

    def clear_images(self, virtual: bool) -> None:
        for chunk in self.output.chunks:
            if isinstance(chunk, ImageOutputChunk):
                identifier = chunk.img_identifiers.pop(virtual, None)
                if identifier is not None:
                    self.canvas.remove_image(identifier)
        self.canvas.present()

    def toggle_virtual_output(self, anchor: Position) -> None:
        if self.virt_hidden:
            # currently suppressed ⇒ un‐suppress and show
            self.virt_hidden = False
            self.show_virtual_output(anchor)
        else:
            # currently visible (or default) ⇒ hide and suppress
            self.clear_virt_output(anchor.bufno)
            # clear_virtual_output already set virt_hidden=True

    def set_win_option(self, option: str, value) -> None:
        if self.display_win:
            self.nvim.api.set_option_value(
                option,
                value,
                {"scope": "local", "win": self.display_win.handle},
            )

    def build_output_text(self, shape, buf: int, virtual: bool) -> Tuple[List[str], int]:
        lines = []
        text = ""
        remaining = self.options.limit_output_chars

        def flush_text():
            nonlocal text, remaining
            if not text:
                return
            value = render_control_chars(text)
            if self.options.limit_output_chars:
                if len(value) > remaining:
                    value = value[:remaining] + f"\n...truncated to {self.options.limit_output_chars} chars\n"
                remaining = max(0, remaining - len(text))
            value, _ = TextOutputChunk(value).place(
                buf, self.options, 0, 0, shape, self.canvas, True
            )
            block = value.rstrip("\n").split("\n")
            # The preview limit applies to text runs, never image padding.
            text_limit = max(1, self.options.virt_text_max_lines - 2)  # header and footer
            if virtual and len(block) > text_limit:
                block = self.truncate_lines(block, text_limit)
            lines.extend(block)
            text = ""

        for chunk in self.output.chunks:
            if isinstance(chunk, ImageOutputChunk):
                flush_text()
                row = len(lines) + 1  # status header
                value, _ = chunk.place(
                    buf, self.options, 0, shape[1] if virtual else row,
                    shape, self.canvas, virtual,
                    winnr=self.nvim.current.window.handle if virtual else None,
                    render_offset_top=row if virtual else 0,
                )
                if virtual in chunk.img_identifiers:
                    lines.extend(value.rstrip("\n").split("\n") if value else [])
                else:
                    text += value
                    flush_text()
            else:
                value, _ = chunk.place(buf, self.options, 0, 0, shape, self.canvas, False)
                text += value
        flush_text()

        header_line = self._get_header_text(self.output)
        block_width = max(1, shape[2])
        header_line = header_line[:block_width]
        lines = [header_line + ("─" * max(0, block_width - len(header_line)))] + [
            line.ljust(block_width) for line in lines
        ]
        lines.append("╰" + ("─" * max(0, block_width - 1)))
        return lines, len(lines) - 1

    def show_virtual_output(self, anchor: Position) -> None:
        if self.virt_hidden:
            return
        if self.nvim.current.buffer.number != anchor.bufno:
            return
        if self.display_win is not None and self.display_win.valid:
            return
        offset = self.calculate_offset(anchor) if self.options.cover_empty_lines else 0

        buf = self.nvim.buffers[anchor.bufno]

        win = self.nvim.current.window
        win_info = self.nvim.funcs.getwininfo(win.handle)[0]
        win_col = win_info["wincol"]
        win_row = anchor.lineno + offset
        win_width = win_info["width"] - win_info["textoff"]
        win_height = win_info["height"]
        if win_width <= 0:
            self.clear_images(True)
            return
        last = self.nvim.funcs.line("$")

        if self.options.virt_lines_off_by_1 and win_row < last - 1:
            win_row += 1

        win_row = min(max(0, win_row), last - 1)

        shape = (
            win_col,
            win_row,
            win_width,
            win_height,
        )
        layout_key = (win.handle, shape, self.canvas.layout_key())
        if (self.output.status == self.displayed_status == OutputStatus.DONE and self.virt_text_id is not None
                and self._virtual_shape == layout_key):
            return
        self._virtual_shape = layout_key
        self.displayed_status = self.output.status
        lines, _ = self.build_output_text(shape, anchor.bufno, True)

        self.virt_text_id = buf.api.set_extmark(
            self.extmark_namespace,
            win_row,
            0,
            {
                **({"id": self.virt_text_id} if self.virt_text_id is not None else {}),
                "virt_lines": [[(line, self.options.hl.virtual_text)] for line in lines],
            },
        )
        self.canvas.present()

    def calculate_offset(self, anchor: Position) -> int:
        offset = 0
        lineno = anchor.lineno
        while lineno > 0:
            current_line = self.nvim.funcs.nvim_buf_get_lines(
                anchor.bufno,
                lineno,
                lineno + 1,
                False,
            )[0]
            is_comment = False
            for x in self.options.cover_lines_starting_with:
                if current_line.startswith(x):
                    is_comment = True
                    break
            if current_line != "" and not is_comment:
                return offset
            else:
                lineno -= 1
                offset -= 1
        # Only get here if current_pos.lineno == 0
        return 0

    def show_floating_win(self, anchor: Position) -> None:
        win = self.nvim.current.window
        previous_cursor = (self.display_win.cursor if self.display_win is not None
                           and self.display_win.valid else None)
        was_at_bottom = previous_cursor is not None and previous_cursor[0] == len(self.display_buf)
        self.source_window = win.handle
        if previous_cursor is None:
            self.source_cursor = win.cursor
        self._float_shape = self.float_shape()
        win_col = 0
        offset = 0
        if self.options.cover_empty_lines:
            offset = self.calculate_offset(anchor)
            win_row = self._buffer_to_window_lineno(anchor.lineno + offset) + 1
        else:
            win_row = self._buffer_to_window_lineno(anchor.lineno + 1)

        # A selected cell's end can be off-screen behind earlier virtual output.
        # The full-output command still needs a usable viewer in that case.
        win_row = max(0, win_row)
        win_width = win.width
        win_height = win.height

        border_w, border_h = border_size(self.options.output_win_border)

        win_height = min(win_height - border_h, self.options.output_win_max_height)
        win_width = min(win_width - border_w, self.options.output_win_max_width)
        if win_width < 1 or win_height < 1:
            return

        # Clear buffer:
        self.display_buf.api.set_lines(0, -1, False, [])

        sign_col_width = 0
        text_off = self.nvim.funcs.getwininfo(win.handle)[0]["textoff"]
        if not self.options.output_win_cover_gutter:
            sign_col_width = text_off

        if win_width - sign_col_width < 1:
            return
        shape = (
            win_col + sign_col_width,
            win_row,
            win_width - sign_col_width,
            win_height,
        )
        lines, real_height = self.build_output_text(shape, self.display_buf.number, False)

        # You can't append lines normally, there will be a blank line at the top
        self.display_buf[0] = lines[0]
        self.display_buf.append(lines[1:])
        self.nvim.api.set_option_value(
            "filetype", "ipynb_output", {"buf": self.display_buf.handle}
        )

        # Open output window
        # assert self.display_window is None
        border = self.options.output_win_border
        zindex = self.options.output_win_zindex
        max_height = min(real_height + 1, self.options.output_win_max_height)
        height = min(win_height, max_height)
        win_row = min(win_row, win.height - border_h - height)

        cropped = False
        if height == win_height and max_height > height:  # It didn't fit on the screen
            if self.options.output_crop_border and type(border) is list:
                cropped = True
                # Expand the border, so top and bottom can change independently
                border = [border[i % len(border)] for i in range(8)]
                border[5 % len(border)] = ""

        if self.options.use_border_highlights:
            border = self.set_border_highlight(border)

        win_opts = {
            "relative": "win",
            "row": max(0, win_row),
            "col": shape[0],
            "width": min(shape[2], self.options.output_win_max_width),
            "height": height,
            "border": border,
            "focusable": True,
            "zindex": zindex,
        }
        if self.options.output_win_style:
            win_opts["style"] = self.options.output_win_style
        if (
            self.options.output_show_more
            and not cropped
            and len(self.display_buf) > height
        ):
            # the entire window size is shown, but the buffer still has more lines to render
            hidden_lines = len(self.display_buf) - height
            if self.options.output_win_cover_gutter and type(border) == list:
                border_pad = border[5 % len(border)][0] * text_off
                win_opts["footer"] = [
                    (border_pad, border[5 % len(border)][1]),
                    (f" 󰁅 {hidden_lines} More Lines ", self.options.hl.foot),
                ]
            else:
                win_opts["footer"] = [(f" 󰁅 {hidden_lines} More Lines ", self.options.hl.foot)]
            win_opts["footer_pos"] = "left"

        if self.display_win is None or not self.display_win.valid:  # open a new window
            window: Window = self.nvim.api.open_win(
                self.display_buf.number,
                False,
                win_opts,
            )
            self.display_win = window

            hl = self.options.hl
            self.set_win_option("winhighlight", f"Normal:{hl.win},NormalNC:{hl.win_nc}")
            # TODO: Refactor once IpynbOutputWindowOpen autocommand is a thing.
            # note, the above setting will probably stay there, just so users can set highlights
            # with their other highlights
            # Text is already wrapped to the measured content width. Inherited
            # gutters, listchars and wrapping invalidate that row accounting.
            self.set_win_option("wrap", False)
            self.set_win_option("number", False)
            self.set_win_option("relativenumber", False)
            self.set_win_option("list", False)
            self.set_win_option("signcolumn", "no")
            self.set_win_option("foldcolumn", "0")
            self.set_win_option("foldenable", False)
            self.set_win_option("winbar", "")
            self.set_win_option("colorcolumn", "")
            self.set_win_option("statuscolumn", "")
            self.set_win_option("winblend", 0)
            self.set_win_option("cursorline", False)
        else:  # move the current window
            self.display_win.api.set_config(win_opts)
        # Clear the inline placement before presenting its floating counterpart.
        self.clear_images(True)
        self.canvas.present()

        if self.display_virt_lines is not None:
            del self.display_virt_lines
            self.display_virt_lines = None

        if self.options.output_virt_lines or self.options.cover_empty_lines:
            virt_lines_y = anchor.lineno
            if self.options.cover_empty_lines:
                virt_lines_y += offset
            virt_lines_height = max_height + border_h
            if self.options.virt_lines_off_by_1:
                virt_lines_y += 1
                virt_lines_height -= 1
            self.display_virt_lines = DynamicPosition(
                self.nvim, self.extmark_namespace, anchor.bufno, virt_lines_y, 0
            )
            self.display_virt_lines.set_height(virt_lines_height)

        if previous_cursor is not None:
            row = len(self.display_buf) if was_at_bottom else min(previous_cursor[0], len(self.display_buf))
            self.display_win.api.set_cursor((row, 0))
        elif self.options.floating_window_focus == "top":
            self.display_win.api.set_cursor((1, 0))

        elif self.options.floating_window_focus == "bottom":
            self.display_win.api.set_cursor((len(self.display_buf), 0))

    def float_shape(self):
        win = self.nvim.current.window
        info = self.nvim.funcs.getwininfo(win.handle)[0]
        return (win.handle, win.width, win.height, info["topline"], info["textoff"],
                self.canvas.layout_key())

    def float_needs_layout(self):
        return self._float_shape != self.float_shape()

    def set_border_highlight(self, border):
        hl = self.options.hl.border_norm
        if not self.output.success:
            hl = self.options.hl.border_fail
        elif self.output.status == OutputStatus.DONE:
            hl = self.options.hl.border_succ

        if type(border) == str:
            notify_error(
                self.nvim,
                "`use_border_highlights` only works when `output_win_border` is specified as a table",
            )
            return border

        for i in range(len(border)):
            match border[i]:
                case [str(_), *_]:
                    border[i][1] = hl
                case str(_):
                    border[i] = [border[i], hl]

        return border

    def remove_window_footer(self) -> None:
        if self.display_win is not None:
            self.display_win.api.set_config({"footer": ""})


def border_size(border: Union[str, List[str], List[List[str]]]):
    width, height = 0, 0
    match border:
        case list(b):
            height += border_char_size(1, b)
            height += border_char_size(5, b)
            width += border_char_size(7, b)
            width += border_char_size(3, b)
        case "rounded" | "single" | "double" | "solid":
            height += 2
            width += 2
        case "shadow":
            height += 1
            width += 1
    return width, height


def border_char_size(index: int, border: Union[List[str], List[List[str]]]):
    match border[index % len(border)]:
        case str(ch) | [str(ch), _]:
            return len(ch)
        case _:
            return 0
