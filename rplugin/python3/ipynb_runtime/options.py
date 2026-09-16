import os

from pynvim import Nvim
from typing import Literal, Optional, Union, List
from dataclasses import dataclass

from ipynb_runtime.utils import notify_error


@dataclass
class HL:
    border_norm = "IpynbOutputBorder"
    border_fail = "IpynbOutputBorderFail"
    border_succ = "IpynbOutputBorderSuccess"
    win = "IpynbOutputWin"
    win_nc = "IpynbOutputWinNC"
    foot = "IpynbOutputFooter"
    cell = "IpynbCell"
    virtual_text = "IpynbVirtualText"

    defaults = {
        border_norm: "FloatBorder",
        border_succ: border_norm,
        border_fail: border_norm,
        win: "NormalFloat",
        win_nc: win,
        foot: "FloatFooter",
        cell: "CursorLine",
        virtual_text: "Comment",
    }


class IpynbOptions:
    numpy_legacy_repr: bool
    auto_image_popup: bool
    auto_init_behavior: str
    auto_open_html_in_browser: bool
    auto_open_output: bool
    cover_empty_lines: bool
    cover_lines_starting_with: List[str]
    copy_output: bool
    enter_output_behavior: str
    image_location: str
    image_provider: str
    limit_output_chars: int
    open_cmd: Optional[str]
    output_crop_border: bool
    output_show_exec_time: bool
    output_show_more: bool
    output_virt_lines: bool
    output_win_border: Union[str, List[str]]
    output_win_cover_gutter: bool
    output_win_hide_on_leave: bool
    output_win_max_height: int
    output_win_max_width: int
    output_win_style: Optional[str]
    output_win_zindex: Optional[str]
    save_path: str
    split_direction: str | None
    split_size: int | None
    show_mimetype_debug: bool
    tick_rate: int
    use_border_highlights: bool
    virt_lines_off_by_1: bool
    virt_text_max_lines: int
    virt_text_output: bool
    virt_text_truncate: Literal["top", "bottom"]
    wrap_output: bool
    nvim: Nvim
    hl: HL
    floating_window_focus: Literal["top", "bottom"]

    def __init__(self, nvim: Nvim):
        self.nvim = nvim
        self.hl = HL()
        # fmt: off
        CONFIG_VARS = [
            ("ipynb_numpy_legacy_repr", True),
            ("ipynb_auto_image_popup", False),
            ("ipynb_auto_init_behavior", "init"), # "raise" or "init"
            ("ipynb_auto_open_html_in_browser", False),
            ("ipynb_auto_open_output", True),
            ("ipynb_cover_empty_lines", False),
            ("ipynb_cover_lines_starting_with", []),
            ("ipynb_copy_output", False),
            ("ipynb_enter_output_behavior", "open_then_enter"),
            ("ipynb_image_location", "both"), # "both", "float", "virt"
            ("ipynb_image_provider", "none"),
            ("ipynb_open_cmd", None),
            ("ipynb_output_crop_border", True),
            ("ipynb_output_show_exec_time", True),
            ("ipynb_output_show_more", False),
            ("ipynb_output_virt_lines", False),
            ("ipynb_output_win_border", [ "", "━", "", "" ]),
            ("ipynb_output_win_cover_gutter", True),
            ("ipynb_limit_output_chars", 1000000),
            ("ipynb_output_win_hide_on_leave", True),
            ("ipynb_output_win_max_height", 999999),
            ("ipynb_output_win_max_width", 999999),
            ("ipynb_output_win_style", False),
            ("ipynb_save_path", os.path.join(nvim.funcs.stdpath("data"), "ipynb.nvim")),
            ("ipynb_split_direction", "right"),
            ("ipynb_split_size", 40),
            ("ipynb_show_mimetype_debug", False),
            ("ipynb_tick_rate", 500),
            ("ipynb_use_border_highlights", False),
            ("ipynb_virt_lines_off_by_1", False),
            ("ipynb_virt_text_max_lines", 12),
            ("ipynb_virt_text_output", False),
            ("ipynb_wrap_output", False),
            ("ipynb_output_win_zindex", 50),
            ("ipynb_virt_text_truncate", "bottom"),
            ("ipynb_floating_window_focus", "top"),
        ]
        # fmt: on

        for name, default in CONFIG_VARS:
            setattr(self, name[len("ipynb_"): ], nvim.vars.get(name, default))

    def update_option(self, option: str, value):
        if option.startswith("ipynb_"):
            option = option[len("ipynb_"): ]
        if hasattr(self, option):
            setattr(self, option, value)
        else:
            notify_error(self.nvim, f"Invalid option passed to IpynbUpdateOption: {option}")
