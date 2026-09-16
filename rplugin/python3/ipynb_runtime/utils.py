from pynvim import Nvim


class IpynbException(Exception):
    pass


def nvimui(func):  # type: ignore
    def inner(self, *args, **kwargs):  # type: ignore
        try:
            func(self, *args, **kwargs)
        except IpynbException as err:
            self.nvim.err_write("[Ipynb] " + str(err) + "\n")

    return inner


def _notify(nvim: Nvim, msg: str, log_level: str) -> None:
    lua = """
        vim.schedule_wrap(function(message, level)
            vim.notify("[Ipynb] " .. message, vim.log.levels[level], {})
        end)(...)
    """
    nvim.exec_lua(lua, msg, log_level)


def notify_info(nvim: Nvim, msg: str) -> None:
    """Use the vim.notify API to display an info message."""
    _notify(nvim, msg, "INFO")


def notify_warn(nvim: Nvim, msg: str) -> None:
    """Use the vim.notify API to display a warning message."""
    _notify(nvim, msg, "WARN")


def notify_error(nvim: Nvim, msg: str) -> None:
    """Use the vim.notify API to display an error message."""
    _notify(nvim, msg, "ERROR")
