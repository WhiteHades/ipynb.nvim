"""Core Rust protocol check: real execution, saved state, stdin, and cleanup."""
import itertools
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("IPYNB_TEST_ENGINE", ROOT / "native/target/release/ipynb-engine"))
PYTHON = os.environ.get("IPYNB_TEST_PYTHON", sys.executable)
(ROOT / ".tmp").mkdir(exist_ok=True)

with tempfile.TemporaryDirectory(dir=ROOT / ".tmp", prefix="native-") as directory:
    with tempfile.TemporaryFile(mode="w+") as errors:
        process = subprocess.Popen([str(BINARY), "--python", PYTHON, "--converter", str(ROOT / "native/converter.py")],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors, text=True)
        messages = queue.Queue()
        def read():
            for line in process.stdout:
                messages.put(json.loads(line))
            messages.put({"event": "eof"})
        threading.Thread(target=read, daemon=True).start()
        ids = itertools.count(1)
        cells = {}
        def receive():
            message = messages.get(timeout=30)
            assert message.get("event") not in ("eof", "error"), message
            if message.get("event") == "cells":
                cells[message["buf"]] = message["cells"]
            if message.get("event") == "input":
                process.stdin.write(json.dumps({"id": next(ids), "method": "stdin", "params": {
                    "kernel": message["kernel"], "value": "answer"}}) + "\n")
                process.stdin.flush()
            return message
        def request(method, params=None, failure=False):
            identity = next(ids)
            process.stdin.write(json.dumps({"id": identity, "method": method, "params": params or {}}) + "\n")
            process.stdin.flush()
            while True:
                response = receive()
                if response.get("id") == identity:
                    if failure:
                        assert "error" in response, response
                    else:
                        assert "error" not in response, response
                    return response.get("result")
        try:
            request("configure", {"image_provider": "none", "numpy_legacy_repr": False})
            kernel = request("init", {"buf": 1, "kernel": "python3"})
            request("init", {"buf": 2, "kernel": kernel, "shared": True})
            source = 'from IPython.display import display\nprint(input("value: "))\ndisplay({"text/plain": "rich", "application/x-test": {"kept": True}}, raw=True)'
            lines = source.split("\n")
            cell_id = request("execute", {"buf": 1, "kernel": kernel, "begin": [0, 0],
                "end": [len(lines)-1, len(lines[-1])], "source": source})
            deadline = time.monotonic() + 30
            while not any(c["id"] == cell_id and c["status"] == "done" for c in cells.get(1, [])):
                assert time.monotonic() < deadline, "execution did not finish"
                receive()
            state_path = str(Path(directory) / "state.json")
            request("save", {"buf": 1, "kernel": kernel, "path": state_path, "lines": lines})
            saved = json.loads(Path(state_path).read_text())
            assert saved["cells"][0]["chunks"][0]["data"]["text/plain"] == "answer\n"
            assert saved["cells"][0]["chunks"][1]["data"]["application/x-test"] == {"kept": True}
            request("deinit", {"buf": 1})
            assert request("state")["kernels"] == [kernel], "shared detach stopped the other buffer"
            request("deinit", {"buf": 2})
            assert request("state")["kernels"] == []
            request("load", {"buf": 1, "path": state_path, "lines": ["changed"]}, failure=True)
            assert request("state")["kernels"] == [], "failed load started a kernel"
            request("load", {"buf": 1, "path": state_path, "lines": lines})
            request("save", {"buf": 1, "path": state_path, "lines": lines})
            assert json.loads(Path(state_path).read_text()) == saved, "saved-state roundtrip lost output data"
            request("shutdown")
            assert process.wait(timeout=10) == 0
        finally:
            if process.poll() is None:
                process.stdin.close()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            errors.seek(0)
            error_text = errors.read()
            if error_text:
                print(error_text, file=sys.stderr)
print("pass: native execution, stdin, shared detach, saved-state validation and roundtrip")
