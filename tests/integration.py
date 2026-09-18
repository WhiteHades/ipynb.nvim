"""exercise notebook execution and persistence in a real, isolated neovim."""
import json
import os
from pathlib import Path
import tempfile
import time

import nbformat
import pynvim

ROOT = Path(__file__).resolve().parents[1]
(ROOT / ".tmp").mkdir(exist_ok=True)


def wait(nvim, predicate, label):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        if predicate():
            return
        nvim.eval('1')
        time.sleep(.1)
    raise AssertionError(label + '\n' + nvim.command_output('messages'))


def launch(path):
    nvim = pynvim.attach('child', argv=['nvim', '--embed', '--headless', '-u',
                                      os.environ.get('IPYNB_TEST_INIT', str(ROOT / 'tests/minimal.lua')), str(path)])
    wait(nvim, lambda: nvim.exec_lua('return vim.b.notebook_kernel_initialized == true'), 'kernel init')
    assert nvim.funcs.exists('*IpynbNotebookRead') == 1, 'notebook open bypassed the Python host'
    return nvim


def close(nvim):
    try:
        nvim.command('qa!')
    except EOFError:
        pass
    nvim.close()


def output_controls(nvim):
    source = nvim.current.buffer.number
    nvim.current.window.cursor = (next(i + 1 for i, line in enumerate(nvim.current.buffer[:])
                                       if line.startswith('print(')), 0)
    nvim.command('doautocmd CursorMoved')
    nvim.input('\\o')
    wait(nvim, lambda: nvim.current.buffer.number != source, 'open output')
    assert 'line 39' in '\n'.join(nvim.current.buffer[:])
    nvim.command('normal! G')
    assert nvim.current.window.cursor[0] == len(nvim.current.buffer[:])
    nvim.input('\\O')
    wait(nvim, lambda: nvim.current.buffer.number == source, 'close output')
    assert not any(nvim.api.win_get_config(w)['relative'] for w in nvim.windows)


with tempfile.TemporaryDirectory(dir=ROOT / '.tmp', prefix='integration-') as temp:
    notebook = Path(temp) / 'check.ipynb'
    nbformat.write(nbformat.v4.new_notebook(cells=[nbformat.v4.new_code_cell(
        'import numpy as np\nassert repr(np.uint8(7)) == "7"\n'
        'assert repr(np.float32(0.5)) == "0.5"\n'
        'assert np.uint8(7).dtype == np.dtype("uint8")\n'
        'print("\\n".join(f"line {i}" for i in range(40)))',
        execution_count=7,
        outputs=[nbformat.v4.new_output('stream', name='stdout',
                 text='\n'.join(f'line {i}' for i in range(40)) + '\n')])]), notebook)
    original_outputs = json.loads(notebook.read_text())['cells'][0]['outputs']
    nvim = launch(notebook)
    try:
        output_controls(nvim)
        nvim.command('write')
        before = json.loads(notebook.read_text())['cells'][0]['outputs']
        assert before == original_outputs
        nvim.input('\\r')
        def saved_execution():
            nvim.eval('IpynbTick()')
            nvim.command('write')
            cell = json.loads(notebook.read_text())['cells'][0]
            errors = [o for o in cell['outputs'] if o['output_type'] == 'error']
            assert not errors, errors
            return cell['execution_count'] != 7 and 'line 39' in str(cell['outputs'])
        wait(nvim, saved_execution, 'execute through mapping and save')
        output_controls(nvim)
        nvim.command('write')
        saved = json.loads(notebook.read_text())['cells'][0]
        # exercise the same actions exposed by the interactive menu.
        nvim.exec_lua("""
          local select = vim.ui.select
          vim.ui.select = function(items, _, callback)
            for _, item in ipairs(items) do
              if item[1] == 'run and advance' then callback(item); return end
            end
            error('missing notebook action')
          end
          vim.cmd('Ipynb')
          vim.ui.select = select
        """)
        assert nvim.current.buffer[nvim.current.window.cursor[0] - 1] == ''
        nvim.current.buffer[nvim.current.window.cursor[0] - 1] = 'print("next cell")'
        nvim.input('\\r')
        def second_execution():
            nvim.eval('IpynbTick()')
            nvim.command('write')
            cells = json.loads(notebook.read_text())['cells']
            code_cells = [cell for cell in cells if cell['cell_type'] == 'code']
            return len(code_cells) == 2 and 'next cell' in str(code_cells[1]['outputs'])
        wait(nvim, second_execution, 'run and advance appended executable cell')
        nvim.input('\\m')
        nvim.eval('1')
        nvim.command('write')
        assert json.loads(notebook.read_text())['cells'][-1]['cell_type'] == 'markdown'
        saved = json.loads(notebook.read_text())['cells'][0]
        nvim.input('\\i')
        nvim.eval('1')
        nvim.command('write')
        assert json.loads(notebook.read_text())['cells'][0]['outputs'] == saved['outputs']
        nvim.command('enew')
        nvim.command('setfiletype markdown')
        assert not nvim.eval("maparg('\\o', 'n')")
        assert nvim.current.buffer.options['buftype'] == ''
    finally:
        close(nvim)
    nvim = launch(notebook)
    try:
        output_controls(nvim)
        nvim.command('write')
        reopened = json.loads(notebook.read_text())['cells'][0]
        assert reopened['execution_count'] == saved['execution_count']
        assert reopened['outputs'] == saved['outputs']
    finally:
        close(nvim)
print('pass: imported and executed output controls, save/reopen, ordinary markdown isolation')
