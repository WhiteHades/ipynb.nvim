import json
from typing import Any, Dict, List, Optional, Tuple
from itertools import chain

import pynvim
from pynvim.api import Buffer
from ipynb_runtime.code_cell import CodeCell
from ipynb_runtime.images import Canvas, get_canvas_given_provider, WeztermCanvas
from ipynb_runtime.info_window import create_info_window
from ipynb_runtime.ipynb import export_outputs, get_default_import_export_file, import_outputs
from ipynb_runtime.save_load import (
    IpynbIOError,
    get_default_save_file,
    load,
    save,
    write_save_file,
)
from ipynb_runtime.moltenbuffer import IpynbKernel
from ipynb_runtime.options import IpynbOptions
from ipynb_runtime.outputbuffer import OutputBuffer
from ipynb_runtime.position import DynamicPosition, Position
from ipynb_runtime.runtime import get_available_kernels
from ipynb_runtime.utils import IpynbException, notify_error, notify_info, notify_warn, nvimui
from pynvim import Nvim


@pynvim.plugin
class Ipynb:
    """The plugin class. Provides an interface for interacting with the plugin via vim functions,
    user commands and user autocommands.

    Invariants that must be maintained in order for this plugin to work:
    - Any CodeCell which belongs to some IpynbKernel _a_ never overlaps with any CodeCell which
      belongs to some IpynbKernel _b_.
    """

    nvim: Nvim
    canvas: Optional[Canvas]
    initialized: bool

    highlight_namespace: int
    extmark_namespace: int

    timer: Optional[int]
    input_timer: Optional[int]

    options: IpynbOptions

    # list of nvim buf numbers to a list of IpynbKernels 'attached' to that buffer
    buffers: Dict[int, List[IpynbKernel]]
    # list of kernel names to the IpynbKernel object that handles that kernel
    # duplicate names are sufixed with (n)
    ipynb_kernels: Dict[str, IpynbKernel]

    def __init__(self, nvim: Nvim):
        self.nvim = nvim
        self.initialized = False

        self.canvas = None
        self.buffers = {}
        self.timer = None
        self.input_timer = None
        self.ipynb_kernels = {}

    def _initialize(self) -> None:
        assert not self.initialized

        self.options = IpynbOptions(self.nvim)

        self.canvas = get_canvas_given_provider(self.nvim, self.options)
        self.canvas.init()

        self.highlight_namespace = self.nvim.funcs.nvim_create_namespace("ipynb-highlights")
        self.extmark_namespace = self.nvim.funcs.nvim_create_namespace("ipynb-extmarks")

        self.timer = self.nvim.eval(
            f"timer_start({self.options.tick_rate}, 'IpynbTick', {{'repeat': -1}})"
        )  # type: ignore

        self.input_timer = self.nvim.eval(
            f"timer_start({self.options.tick_rate}, 'IpynbTickInput', {{'repeat': -1}})"
        )  # type: ignore

        self._setup_highlights()
        self._set_autocommands()

        self.nvim.exec_lua("_ipynb_prompt_init = require('ipynb.molten_prompt').prompt_init")
        self.nvim.exec_lua("_ipynb_select_and_run = require('ipynb.molten_prompt').select_and_run")
        self.nvim.exec_lua("_ipynb_prompt_init_and_run = require('ipynb.molten_prompt').prompt_init_and_run")

        self.initialized = True

    def _set_autocommands(self) -> None:
        self.nvim.command("augroup ipynb_runtime")
        self.nvim.command("autocmd CursorMoved  * call IpynbOnCursorMoved()")
        self.nvim.command("autocmd CursorMovedI * call IpynbOnCursorMoved()")
        self.nvim.command("autocmd WinScrolled  * call IpynbOnWinScrolled()")
        self.nvim.command("autocmd BufEnter     * call IpynbUpdateInterface()")
        self.nvim.command("autocmd BufLeave     * call IpynbBufLeave()")
        self.nvim.command("autocmd BufUnload    * call IpynbOnBufferUnload()")
        self.nvim.command("autocmd ExitPre      * call IpynbOnExitPre()")
        self.nvim.command("augroup END")

    def _setup_highlights(self) -> None:
        self.nvim.exec_lua("_ipynb_hl_utils = require('ipynb.molten_hl_utils')")
        hl_utils = self.nvim.lua._ipynb_hl_utils
        hl_utils.set_default_highlights(self.options.hl.defaults)

    def _deinitialize(self) -> None:
        seen = set()
        for ipynb_kernels in self.buffers.values():
            for ipynb_kernel in ipynb_kernels:
                if id(ipynb_kernel) in seen:
                    continue
                seen.add(id(ipynb_kernel))
                ipynb_kernel.deinit()

        self.buffers.clear()
        self.ipynb_kernels.clear()
        if self.canvas is not None:
            self.canvas.deinit()
        if self.timer is not None:
            self.nvim.funcs.timer_stop(self.timer)
        if self.input_timer is not None:
            self.nvim.funcs.timer_stop(self.input_timer)

    def _initialize_if_necessary(self) -> None:
        if not self.initialized:
            self._initialize()

    def _get_current_buf_kernels(self, requires_instance: bool) -> Optional[List[IpynbKernel]]:
        self._initialize_if_necessary()

        maybe_molten = self.buffers.get(self.nvim.current.buffer.number)
        if requires_instance and (maybe_molten is None or len(maybe_molten) == 0):
            raise IpynbException(
                "Ipynb is not initialized in this buffer; run `:IpynbInit` to initialize."
            )
        return maybe_molten

    def _clear_on_buf_leave(self, buffer_number: Optional[int] = None) -> None:
        if not self.initialized:
            return

        for ipynb_kernels in self.buffers.values():
            for ipynb_kernel in ipynb_kernels:
                ipynb_kernel.clear_interface(buffer_number)
                ipynb_kernel.clear_open_output_windows(buffer_number)

    def _clear_interface(
        self,
        ipynb_kernels: list[IpynbKernel] | None = None,
        buffer_number: Optional[int] = None,
    ) -> None:
        if not self.initialized:
            return

        if ipynb_kernels is not None:
            for ipynb_kernel in ipynb_kernels:
                ipynb_kernel.clear_virt_outputs(buffer_number)

        else:
            # NOTE: I think this is wrong, because it disables all available kernels, instead of just shutting down the one, that is being called to close.

            for ipynb_kernels in self.buffers.values():
                for ipynb_kernel in ipynb_kernels:
                    ipynb_kernel.clear_virt_outputs(buffer_number)

        self._clear_on_buf_leave(buffer_number)

    def _update_interface(self) -> None:
        """Called on load, show_output/hide_output and buf enter"""
        if not self.initialized:
            return

        ipynb_kernels = self._get_current_buf_kernels(False)
        if ipynb_kernels is None:
            return

        for m in ipynb_kernels:
            m.update_interface()

    def _on_cursor_moved(self, scrolled=False) -> None:
        if not self.initialized:
            return

        ipynb_kernels = self._get_current_buf_kernels(False)
        if ipynb_kernels is None:
            return

        for m in ipynb_kernels:
            m.on_cursor_moved(scrolled)

    def _initialize_buffer(self, kernel_name: str, shared=False) -> IpynbKernel | None:
        assert self.canvas is not None
        if shared:  # use an existing molten kernel, for a new neovim buffer
            molten = self.ipynb_kernels.get(kernel_name)
            if molten is not None:
                molten.add_nvim_buffer(self.nvim.current.buffer)
                self.buffers[self.nvim.current.buffer.number] = [molten]
                return molten

            notify_warn(
                self.nvim,
                f"No running kernel {kernel_name} to share. Continuing with a new kernel.",
            )

        kernel_id = kernel_name
        if self.ipynb_kernels.get(kernel_name) is not None:
            kernel_id = f"{kernel_name}_{len(self.ipynb_kernels)}"

        try:
            molten = IpynbKernel(
                self.nvim,
                self.canvas,
                self.highlight_namespace,
                self.extmark_namespace,
                self.nvim.current.buffer,
                self.options,
                kernel_name,
                kernel_id,
            )

            self.add_kernel(self.nvim.current.buffer, kernel_id, molten)
            molten._doautocmd("IpynbInitPost")
            if isinstance(self.canvas, WeztermCanvas):
                self.canvas.wezterm_split()

            return molten
        except Exception as e:
            notify_error(
                self.nvim, f"Could not initialize kernel named '{kernel_name}'.\nCaused By: {e}"
            )

    def add_kernel(self, buffer: Buffer, kernel_id: str, kernel: IpynbKernel):
        """Add a new IpynbKernel to be tracked by Ipynb.
        - Adds the new kernel to the buffer list for the given buffer
        - Adds the new kernel to the ipynb_kernels list, with a suffix if the name is already taken
        """
        if self.buffers.get(buffer.number) is None:
            self.buffers[buffer.number] = [kernel]
        else:
            self.buffers[buffer.number].append(kernel)

        self.ipynb_kernels[kernel_id] = kernel

    @pynvim.command("IpynbInit", nargs="*", sync=True, complete="file")  # type: ignore
    @nvimui  # type: ignore
    def command_init(self, args: List[str]) -> None:
        self._initialize_if_necessary()

        shared = False
        if len(args) > 0 and args[0] == "shared":
            shared = True
            args = args[1:]

        if len(args) > 0:
            kernel_name = args[0]
            self._initialize_buffer(kernel_name, shared=shared)
        else:
            PROMPT = "Select the kernel to launch:"
            available_kernels = [(x, False) for x in get_available_kernels()]
            running_kernels = [(x, True) for x in self.ipynb_kernels.keys()]

            if shared:
                # only show running kernels
                available_kernels = []

            kernels = available_kernels + running_kernels
            if len(kernels) == 0:
                notify_error(
                    self.nvim, f"Unable to find any {'shared' if shared else ''}kernels to launch."
                )
                return

            self.nvim.lua._ipynb_prompt_init(kernels, PROMPT)

    def _deinit_buffer(
        self, ipynb_kernels: List[IpynbKernel], buffer_number: Optional[int] = None
    ) -> None:
        """Detach kernels from one buffer and stop those with no remaining clients."""
        if buffer_number is None:
            buffer_number = self.nvim.current.buffer.number

        attached = self.buffers.get(buffer_number)
        for kernel in list(ipynb_kernels):
            if attached is not None and kernel in attached:
                attached.remove(kernel)

            kernel.clear_buffer(buffer_number)
            kernel.remove_nvim_buffer(buffer_number)
            if kernel.buffers:
                continue

            kernel.deinit()
            self.ipynb_kernels.pop(kernel.kernel_id, None)

        if attached is not None and not attached:
            del self.buffers[buffer_number]

    def _do_evaluate_expr(self, kernel_name: str, expr):
        self._initialize_if_necessary()

        kernels = self._get_current_buf_kernels(True)
        assert kernels is not None

        kernel = None
        for k in kernels:
            if k.kernel_id == kernel_name:
                kernel = k
                break
        if kernel is None:
            raise IpynbException(f"Kernel {kernel_name} not found")

        bufno = self.nvim.current.buffer.number
        cell = CodeCell(
            self.nvim,
            DynamicPosition(self.nvim, self.extmark_namespace, bufno, 0, 0),
            DynamicPosition(self.nvim, self.extmark_namespace, bufno, 0, 0, right_gravity=True),
        )

        kernel.run_code(expr, cell)

    def _get_sorted_buf_cells(self, kernels: List[IpynbKernel], bufnr: int) -> List[CodeCell]:
        return sorted([x for x in chain(*[k.outputs.keys() for k in kernels]) if x.bufno == bufnr])

    @pynvim.command("IpynbDeinit", nargs=0, sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_deinit(self) -> None:
        self._initialize_if_necessary()

        kernels = self._get_current_buf_kernels(True)

        # NOTE: Assert is generally a bad idea to use. Instead it is better to call custom error, or something like that.
        # Also, assert can be disabled with `-O` or `-OO` flags.
        # This is not a production solution
        assert kernels is not None

        self._clear_interface(kernels, self.nvim.current.buffer.number)

        self._deinit_buffer(kernels, self.nvim.current.buffer.number)

    @pynvim.command("IpynbInfo", nargs=0, sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_info(self) -> None:
        create_info_window(self.nvim, self.ipynb_kernels, self.buffers, self.initialized)

    def _do_evaluate(self, kernel_name: str, pos: Tuple[Tuple[int, int], Tuple[int, int]]) -> None:
        self._initialize_if_necessary()

        kernels = self._get_current_buf_kernels(True)
        assert kernels is not None

        kernel = None
        for k in kernels:
            if k.kernel_id == kernel_name:
                kernel = k
                break
        if kernel is None:
            raise IpynbException(f"Kernel {kernel_name} not found")

        bufno = self.nvim.current.buffer.number
        span = CodeCell(
            self.nvim,
            DynamicPosition(self.nvim, self.extmark_namespace, bufno, *pos[0]),
            DynamicPosition(self.nvim, self.extmark_namespace, bufno, *pos[1], right_gravity=True),
        )

        code = span.get_text(self.nvim)

        # delete overlapping cells from other kernels. Maintains the invariant that all code cells
        # from different kernels are disjoint
        for k in kernels:
            if k.kernel_id != kernel.kernel_id:
                if not k.try_delete_overlapping_cells(span):
                    return

        kernel.run_code(code, span)

    @pynvim.function("IpynbUpdateOption", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def function_update_option(self, args) -> None:
        self._initialize_if_necessary()

        if len(args) == 2:
            option, value = args
            self.options.update_option(option, value)
        else:
            notify_error(
                self.nvim,
                f"Wrong number of arguments passed to :IpynbUpdateOption, expected 2, given {len(args)}",
            )

    @pynvim.function("IpynbAvailableKernels", sync=True)  # type: ignore
    def function_available_kernels(self, _):
        """List of string kernel names that molten knows about"""
        return get_available_kernels()

    @pynvim.function("IpynbRunningKernels", sync=True)  # type: ignore
    def function_list_running_kernels(self, args: List[Optional[bool]]) -> List[str]:
        """List all the running kernels. When passed [True], returns only buf local kernels"""
        if not self.initialized:
            return []
        if len(args) > 0 and args[0]:
            buf = self.nvim.current.buffer.number
            if buf not in self.buffers:
                return []
            return [x.kernel_id for x in self.buffers[buf]]
        return list(self.ipynb_kernels.keys())

    @pynvim.function("IpynbStatusLineKernels", sync=True)  # type: ignore
    def function_status_line_kernels(self, args) -> str:
        kernels = self.function_list_running_kernels(args)
        return " ".join(kernels)

    @pynvim.function("IpynbStatusLineInit", sync=True)  # type: ignore
    def function_status_line_init(self, _) -> str:
        if self.initialized:
            return "Ipynb"
        return ""

    @pynvim.command("IpynbNext", sync=True, nargs="*")  # type: ignore
    @nvimui
    def command_next(self, args: List[str]) -> None:
        count = 1
        if len(args) > 0:
            try:
                count = int(args[0])
            except ValueError:
                count = 1

        c = self.nvim.api.win_get_cursor(0)
        bufnr = self.nvim.current.buffer.number
        pos = Position(bufnr, c[0] - 1, c[1])
        kernels = self._get_current_buf_kernels(True)
        assert kernels is not None

        all_cells = self._get_sorted_buf_cells(kernels, bufnr)

        starting_index = None
        match all_cells:
            case [first, *_] if pos < first.begin:
                starting_index = 0
                if count > 0:
                    count -= 1
            case [*_, last] if last.end < pos:
                starting_index = len(all_cells) - 1
                if count < 0:
                    count += 1
            case _:
                for i, cell in enumerate(all_cells):
                    if pos in cell or (
                        i <= len(all_cells) - 2 and pos < all_cells[i + 1].begin and cell.end < pos
                    ):
                        starting_index = i

        if starting_index is not None:
            target_idx = (starting_index + count) % len(all_cells)
            target_pos = all_cells[target_idx].begin
            self.nvim.api.win_set_cursor(0, (target_pos.lineno + 1, target_pos.colno))
        else:
            notify_warn(self.nvim, "No cells to jump to")

    @pynvim.command("IpynbGoto", sync=True, nargs="*")  # type: ignore
    @nvimui
    def command_goto(self, args: List[str]) -> None:
        count = 1
        if len(args) > 0:
            try:
                count = int(args[0])
            except ValueError:
                count = 1

        kernels = self._get_current_buf_kernels(True)
        assert kernels is not None

        all_cells = self._get_sorted_buf_cells(kernels, self.nvim.current.buffer.number)
        if len(all_cells) == 0:
            notify_warn(self.nvim, "No cells to jump to")
            return

        target_pos = all_cells[(count - 1) % len(all_cells)].begin
        self.nvim.api.win_set_cursor(0, (target_pos.lineno + 1, target_pos.colno))

    @pynvim.command("IpynbPrev", sync=True, nargs="*")  # type: ignore
    @nvimui
    def command_prev(self, args: List[str]) -> None:
        count = -1
        if len(args) > 0:
            try:
                count = -int(args[0])
            except ValueError:
                count = -1
        self.command_next([str(count)])

    @pynvim.command("IpynbEnterOutput", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_enter_output_window(self) -> None:
        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None

        # We can do this iff we ensure that different kernels don't contain code cells that overlap
        for kernel in ipynb_kernels:
            kernel.enter_output()

    @pynvim.command("IpynbOpenInBrowser", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_open_in_browser(self) -> None:
        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None

        for kernel in ipynb_kernels:
            if kernel.open_in_browser():
                notify_info(self.nvim, "Opened in browser")
                return

    @pynvim.command("IpynbImagePopup", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_image_popup(self) -> None:
        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None

        for kernel in ipynb_kernels:
            if kernel.open_image_popup():
                return

    @pynvim.command("IpynbEvaluateArgument", nargs="*", sync=True)  # type: ignore
    @nvimui
    def commnand_ipynb_evaluate_argument(self, args: List[str]) -> None:
        if len(args) > 0 and args[0] in map(
            lambda x: x.kernel_id, self.buffers[self.nvim.current.buffer.number]
        ):
            self._do_evaluate_expr(args[0], " ".join(args[1:]))
        else:
            self.kernel_check(
                f"IpynbEvaluateArgument %k {' '.join(args)}", self.nvim.current.buffer
            )

    @pynvim.command("IpynbEvaluateVisual", nargs="*", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_evaluate_visual(self, args) -> None:
        if len(args) > 0:
            kernel = args[0]
        else:
            self.kernel_check("IpynbEvaluateVisual %k", self.nvim.current.buffer)
            return
        _, lineno_begin, colno_begin, _ = self.nvim.funcs.getpos("'<")
        _, lineno_end, colno_end, _ = self.nvim.funcs.getpos("'>")

        if lineno_begin == 0 or colno_begin == 0 or lineno_end == 0 or colno_end == 0:
            notify_error(self.nvim, "No visual selection found")
            return

        span = (
            (
                lineno_begin - 1,
                min(colno_begin, len(self.nvim.funcs.getline(lineno_begin))) - 1,
            ),
            (
                lineno_end - 1,
                min(colno_end, len(self.nvim.funcs.getline(lineno_end))),
            ),
        )

        self._do_evaluate(kernel, span)

    @pynvim.function("IpynbEvaluateRange", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def evaulate_range(self, args) -> None:
        start_col, end_col = 1, 0
        kernel = None
        span = args
        if type(args[0]) == str:
            kernel = args[0]
            span = args[1:]

        if len(span) == 2:
            start_line, end_line = span
        elif len(span) == 4:
            start_line, end_line, start_col, end_col = span
        else:
            notify_error(self.nvim, f"Invalid args passed to IpynbEvaluateRange. Got: {args}")
            return

        if not kernel:
            self.kernel_check(
                f"call IpynbEvaluateRange('%k', {start_line}, {end_line}, {start_col}, {end_col})",
                self.nvim.current.buffer,
            )
            return

        span = (
            (start_line - 1, start_col - 1),
            (end_line - 1, end_col - 1),
        )

        self._do_evaluate(kernel.strip(), span)

    @pynvim.command("IpynbEvaluateOperator", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_evaluate_operator(self) -> None:
        self._initialize_if_necessary()

        self.nvim.options["operatorfunc"] = "IpynbOperatorfunc"
        self.nvim.feedkeys("g@")

    @pynvim.command("IpynbEvaluateLine", nargs="*", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_evaluate_line(self, args: List[str]) -> None:
        _, lineno, _, _, _ = self.nvim.funcs.getcurpos()
        lineno -= 1

        span = ((lineno, 0), (lineno, -1))

        if len(args) > 0 and args[0]:
            self._do_evaluate(args[0], span)
        else:
            self.kernel_check("IpynbEvaluateLine %k", self.nvim.current.buffer)

    def kernel_check(self, command: str, buffer: Buffer) -> None:
        """Figure out if there is more than one kernel attached to the given buffer. If there is,
        prompt the user for the kernel name, and run the given command with the new kernel subbed in
        for %k. If there is no kernel, throw an error. If there is one kernel, use it
        """
        self._initialize_if_necessary()

        kernels = self.buffers.get(buffer.number)
        if not kernels and self.options.auto_init_behavior != "raise":
            available_kernels = [(x, False) for x in get_available_kernels()]
            shared_kernels = [(x, True) for x in self.ipynb_kernels.keys()]
            PROMPT = "You Need to Initialize a Kernel First:"
            self.nvim.lua._ipynb_prompt_init_and_run(available_kernels + shared_kernels, PROMPT, command)
        elif not kernels:  # and auto_init_behavior == "raise"
            raise IpynbException(
                "Ipynb is not initialized in this buffer; run `:IpynbInit` to initialize."
            )
        elif len(kernels) == 1:
            import re
            pat = r'(^|[^\\])%k'
            c = re.sub(pat, lambda x: x[1] + kernels[0].kernel_id, command)
            c = c.replace(r"\%k", "%k") # un-escape escaped chars
            self.nvim.command(c)
        else:
            PROMPT = "Please select a kernel:"
            available_kernels = [kernel.kernel_id for kernel in kernels]
            self.nvim.lua._ipynb_select_and_run(available_kernels, PROMPT, command)

    @pynvim.command("IpynbReevaluateAll", nargs=0, sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_reevaluate_all(self) -> None:
        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None

        for kernel in ipynb_kernels:
            kernel.reevaluate_all()

    @pynvim.command("IpynbReevaluateCell", nargs=0, sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_evaluate_cell(self) -> None:
        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None

        # we can do this iff we ensure that different kernels don't contain code cells that overlap
        in_cell = False
        for kernel in ipynb_kernels:
            if kernel.reevaluate_cell():
                in_cell = True

        if not in_cell:
            notify_error(self.nvim, "Not in a cell")

    @pynvim.command("IpynbInterrupt", nargs="*", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_interrupt(self, args) -> None:
        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None

        if len(args) > 0:
            kernel = args[0]
        else:
            self.kernel_check("IpynbInterrupt %k", self.nvim.current.buffer)
            return

        for molten in ipynb_kernels:
            if molten.kernel_id == kernel:
                molten.interrupt()
                return

        notify_error(self.nvim, f"Unable to find kernel: {kernel}")

    @pynvim.command("IpynbRestart", nargs="*", sync=True, bang=True)  # type: ignore
    @nvimui  # type: ignore
    def command_restart(self, args, bang) -> None:
        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None

        if len(args) > 0:
            kernel = args[0]
        else:
            self.kernel_check(f"IpynbRestart{'!' if bang else ''} %k", self.nvim.current.buffer)
            return

        for molten in ipynb_kernels:
            if molten.kernel_id == kernel:
                molten.restart(delete_outputs=bang)
                return
        notify_error(self.nvim, f"Unable to find kernel: {kernel}")

    @pynvim.command("IpynbDelete", nargs=0, sync=True, bang=True)  # type: ignore
    @nvimui  # type: ignore
    def command_delete(self, bang) -> None:
        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None

        for molten in ipynb_kernels:
            if bang:
                # Delete all cells in current buffer
                molten.clear_buffer(self.nvim.current.buffer.number)
            elif molten.selected_cell is not None:
                molten.delete_current_cell()
                return

    @pynvim.command("IpynbShowOutput", nargs=0, sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_show_output(self) -> None:
        self._initialize_if_necessary()

        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None

        for molten in ipynb_kernels:
            if molten.selected_cell is not None:
                molten.should_show_floating_win = True
                self._update_interface()
                return

    @pynvim.command("IpynbHideOutput", nargs=0, sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_hide_output(self) -> None:
        ipynb_kernels = self._get_current_buf_kernels(False)
        if ipynb_kernels is None:
            # get the current buffer, and then search for it in all molten buffers
            cur_buf = self.nvim.current.buffer
            for moltenbuf in self.buffers.values():
                # if we find it, then we know this is a molten output, and we can safely quit and
                # call hide to hide it
                output_windows = map(
                    lambda x: x.display_buf, chain(*[o.outputs.values() for o in moltenbuf])
                )
                if cur_buf in output_windows:
                    self.nvim.command("q")
                    self.nvim.command(":IpynbHideOutput")
                    return
            return

        for molten in ipynb_kernels:
            molten.should_show_floating_win = False

        self._update_interface()

    @pynvim.command("IpynbImportOutput", nargs="*", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_import(self, args) -> None:
        self._initialize_if_necessary()

        buf = self.nvim.current.buffer
        if len(args) > 0:
            path = args[0]
        else:
            path = get_default_import_export_file(self.nvim, buf)

        if len(args) > 1:
            kernel = args[1]
        else:
            path = path.replace("%k", r"\%k")
            self.kernel_check(f"IpynbImportOutput {path} %k", buf)
            return

        kernels = self._get_current_buf_kernels(True)
        assert kernels is not None
        for molten in kernels:
            if molten.kernel_id == kernel:
                import_outputs(self.nvim, molten, path)
                break

    @pynvim.command("IpynbExportOutput", nargs="*", sync=True, bang=True)  # type: ignore
    @nvimui  # type: ignore
    def command_export(self, args, bang: bool) -> None:
        kernels = self._get_current_buf_kernels(True)
        assert kernels is not None

        buf = self.nvim.current.buffer
        if len(args) > 0:
            path = args[0]
        else:
            path = get_default_import_export_file(self.nvim, buf)

        if len(args) > 1:
            kernel = args[1]
        else:
            path = path.replace("%k", r"\%k")
            self.kernel_check(f"IpynbExportOutput{'!' if bang else ''} {path} %k", buf)
            return

        for molten in kernels:
            if molten.kernel_id == kernel:
                export_outputs(self.nvim, molten, path, bang)
                break

    @pynvim.command("IpynbSave", nargs="*", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_save(self, args) -> None:
        kernels = self._get_current_buf_kernels(True)
        assert kernels is not None

        buf = self.nvim.current.buffer
        if len(args) > 0:
            path = args[0]
        else:
            path = get_default_save_file(self.options, buf)

        if len(args) > 1:
            kernel = args[1]
        else:
            path = path.replace("%k", r"\%k")
            self.kernel_check(f"IpynbSave {path} %k", buf)
            return

        for molten in kernels:
            if molten.kernel_id == kernel:
                with open(path, "w") as file:
                    write_save_file(path, save(molten, buf.number))
                break
        notify_info(self.nvim, f"Saved kernel `{kernel}` to: {path}")

    @pynvim.command("IpynbLoad", nargs="*", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def command_load(self, args) -> None:
        self._initialize_if_necessary()

        shared = False

        if len(args) > 0 and args[0] == "shared":
            shared = True
            args = args[1:]

        if len(args) > 0:
            path = args[0]
        else:
            path = get_default_save_file(self.options, self.nvim.current.buffer)

        if self.nvim.current.buffer.number in self.buffers:
            raise IpynbException(
                "Ipynb is already initialized for this buffer; IpynbLoad initializes Ipynb."
            )

        with open(path) as file:
            data = json.load(file)

        molten = None

        try:
            notify_info(self.nvim, f"Attempting to load from: {path}")

            IpynbIOError.assert_has_key(data, "version", int)
            if (version := data["version"]) != 1:
                raise IpynbIOError(f"Bad version: {version}")

            IpynbIOError.assert_has_key(data, "kernel", str)
            kernel_name = data["kernel"]

            molten = self._initialize_buffer(kernel_name, shared=shared)
            if molten:
                load(self.nvim, molten, self.nvim.current.buffer, data)

                self._update_interface()
        except IpynbIOError as err:
            if molten is not None:
                self._deinit_buffer([molten], self.nvim.current.buffer.number)

            raise IpynbException("Error while doing Ipynb IO: " + str(err))

    # Internal functions which are exposed to VimScript

    @pynvim.function("IpynbBufLeave", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def function_clear_interface(self, _: List[Any]) -> None:
        self._clear_on_buf_leave()

    @pynvim.function("IpynbOnBufferUnload", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def function_on_buffer_unload(self, _: Any) -> None:
        abuf_str = self.nvim.funcs.expand("<abuf>")
        if not abuf_str:
            return

        molten = self.buffers.get(int(abuf_str))
        if molten is None:
            return

        self._deinit_buffer(molten, int(abuf_str))

    @pynvim.function("IpynbOnExitPre", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def function_on_exit_pre(self, _: Any) -> None:
        self._deinitialize()

    @pynvim.function("IpynbTick", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def function_ipynb_tick(self, _: Any) -> None:
        self._initialize_if_necessary()

        ipynb_kernels = self._get_current_buf_kernels(False)
        if ipynb_kernels is None:
            return

        for m in ipynb_kernels:
            m.tick()

    @pynvim.function("IpynbTickInput", sync=False)  # type: ignore
    @nvimui  # type: ignore
    def function_ipynb_tick_input(self, _: Any) -> None:
        self._initialize_if_necessary()

        ipynb_kernels = self._get_current_buf_kernels(False)
        if ipynb_kernels is None:
            return

        for m in ipynb_kernels:
            m.tick_input()

    @pynvim.function("IpynbSendStdin", sync=False)  # type: ignore
    @nvimui  # type: ignore
    def function_ipynb_send_stdin(self, args: Tuple[str, str]) -> None:
        ipynb_kernels = self._get_current_buf_kernels(False)
        if ipynb_kernels is None:
            return

        for m in ipynb_kernels:
            if m.kernel_id == args[0]:
                m.send_stdin(args[1])

    @pynvim.function("IpynbUpdateInterface", sync=True)  # type: ignore
    @nvimui  # type: ignore
    def function_update_interface(self, _: Any) -> None:
        self._update_interface()

    @pynvim.function("IpynbOnCursorMoved", sync=True)
    @nvimui
    def function_on_cursor_moved(self, _) -> None:
        self._on_cursor_moved()

    @pynvim.function("IpynbOnWinScrolled", sync=True)
    @nvimui
    def function_on_win_scrolled(self, _) -> None:
        self._on_cursor_moved(scrolled=True)

    @pynvim.function("IpynbOperatorfunc", sync=True)
    @nvimui
    def function_ipynb_operatorfunc(self, args) -> None:
        if not args:
            return

        kind = args[0]

        _, lineno_begin, colno_begin, _ = self.nvim.funcs.getpos("'[")
        _, lineno_end, colno_end, _ = self.nvim.funcs.getpos("']")

        if kind == "line":
            colno_begin = 1
            colno_end = 0
        elif kind == "char":
            colno_begin = min(colno_begin, len(self.nvim.funcs.getline(lineno_begin)))
            colno_end = min(colno_end, len(self.nvim.funcs.getline(lineno_end))) + 1
        else:
            raise IpynbException(f"this kind of selection is not supported: '{kind}'")

        span = (
            (lineno_begin, colno_begin),
            (lineno_end, colno_end),
        )

        self.kernel_check(
            f"call IpynbEvaluateRange('%k', {span[0][0]}, {span[1][0]}, {span[0][1]}, {span[1][1]})",
            self.nvim.current.buffer,
        )

    @pynvim.function("IpynbDefineCell", sync=True)
    def function_ipynb_define_cell(self, args: List[int]) -> None:
        if not args:
            return

        ipynb_kernels = self._get_current_buf_kernels(True)
        assert ipynb_kernels is not None
        assert self.canvas is not None

        start = args[0]
        end = args[1]

        if len(args) == 3:
            kernel = args[2]
        elif len(self.buffers[self.nvim.current.buffer.number]) == 1:
            kernel = self.buffers[self.nvim.current.buffer.number][0].kernel_id
        else:
            raise IpynbException(
                "IpynbDefineCell called without a kernel argument while multiple kernels are active"
            )

        bufno = self.nvim.current.buffer.number
        span = CodeCell(
            self.nvim,
            DynamicPosition(self.nvim, self.extmark_namespace, bufno, start - 1, 0),
            DynamicPosition(
                self.nvim, self.extmark_namespace, bufno, end - 1, -1, right_gravity=True
            ),
        )

        for molten in ipynb_kernels:
            if molten.kernel_id == kernel:
                molten.outputs[span] = OutputBuffer(
                    self.nvim, self.canvas, molten.extmark_namespace, self.options
                )
                break

    @pynvim.command("IpynbToggleVirtual", nargs="0", sync=True, bang=True)  # type: ignore
    @nvimui  # type: ignore
    def command_toggle_virtual(self, args: List[Any], bang: bool) -> None:
        """
        Toggle the virtual-text output on/off for the cell under the cursor.
        With a bang (`:IpynbToggleVirtual!`), toggle ALL cells in this buffer.
        """
        kernels = self._get_current_buf_kernels(True)
        assert kernels is not None

        # If called with !, toggle EVERY cell
        if bang:
            # if any cell is currently visible, hide all; otherwise show all
            any_visible = any(
                outbuf.virt_text_id is not None and not outbuf.virt_hidden
                for kern in kernels
                for outbuf in kern.outputs.values()
            )
            for kern in kernels:
                for cell, outbuf in kern.outputs.items():
                    if any_visible:
                        outbuf.clear_virt_output(cell.end.bufno)
                    else:
                        outbuf.virt_hidden = False
                        outbuf.show_virtual_output(cell.end)
            return

        # Otherwise, just toggle the cell under the cursor
        for kern in kernels:
            cell = kern._get_selected_span()
            if cell is not None:
                outbuf = kern.outputs[cell]
                outbuf.toggle_virtual_output(cell.end)
                return

    @pynvim.command("IpynbYankOutput", nargs="0", sync=True, bang=True)  # type: ignore
    @nvimui  # type: ignore
    def command_yank_output(self, _args: List[Any], bang: bool) -> None:
        """
        Yank the output of the cell under the cursor.
        With a bang (`:IpynbYankOutput!`), yank to the system clipboard.
        """
        kernels = self._get_current_buf_kernels(True)
        assert kernels is not None

        for kern in kernels:
            cell = kern._get_selected_span()
            if cell is None or cell not in kern.outputs:
                continue

            outbuf: OutputBuffer = kern.outputs[cell]
            # build the plain-text output (not virtual)
            bufno = self.nvim.current.buffer.number
            # Get window width for proper text wrapping
            win_width = self.nvim.current.window.width
            shape = (0, 0, win_width, 0)  # (win_col, win_row, win_width, win_height)
            lines, _ = outbuf.build_output_text(shape, bufno, False)
            lines = lines[1:]  # Remove header
            if not lines:
                notify_warn(
                    self.nvim,
                    "No output to yank.",
                )
                return

            text = "\n".join(lines)
            # choose register: system clipboard ("+) if bang, else default (")
            reg = "+" if bang else '"'
            # set the register
            self.nvim.funcs.setreg(reg, text)
            return
