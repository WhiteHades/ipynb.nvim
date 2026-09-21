"""Exercise prerequisite checks and install plans without changing the host."""
import os
from pathlib import Path
import shutil
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
BASH = shutil.which("bash")
STUB = '''#!/bin/bash
name=${0##*/}
case "$name" in
  uname) echo "${TEST_OS:-Linux}" ;;
  nvim) echo "NVIM v${NVIM_VERSION:-0.12.0}" ;;
  python3) [[ $1 != --version ]] || echo 'Python 3.10.0' ;;
  rustc) echo "rustc ${RUST_VERSION:-1.85.0}" ;;
  tree-sitter) echo "tree-sitter ${TREE_VERSION:-0.26.1}" ;;
  apt-get|dnf|pacman|brew|cargo)
    if [[ $name == cargo && $1 == --version ]]; then echo "cargo ${RUST_VERSION:-1.85.0}"; exit; fi
    echo "$name $*" >> "$INSTALL_LOG"
    [[ ${FAIL_INSTALL:-0} != 1 ]] ;;
esac
'''


class PrerequisiteTests(unittest.TestCase):
    def setUp(self):
        base = ROOT / '.tmp' / 'prerequisites'
        base.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=base)
        self.addCleanup(self.temp.cleanup)
        self.bin = Path(self.temp.name)
        self.log = self.bin / 'install.log'
        for name in ('uname', 'nvim', 'python3', 'cargo', 'rustc', 'git', 'curl',
                     'tar', 'cc', 'c++', 'tree-sitter', 'uv', 'magick', 'apt-get'):
            path = self.bin / name
            path.write_text(STUB)
            path.chmod(0o755)
        # Non-root test runs still exercise the same fake package manager.
        sudo = self.bin / 'sudo'
        sudo.write_text('#!/bin/bash\nexec "$@"\n')
        sudo.chmod(0o755)

    def run_script(self, *args, **environment):
        env = dict(os.environ, PATH=str(self.bin), INSTALL_LOG=str(self.log),
                   NO_COLOR='1', **environment)
        return subprocess.run([BASH, str(ROOT / 'scripts/prerequisites.sh'), *args],
                              env=env, text=True, capture_output=True, stdin=subprocess.DEVNULL)

    def test_check_is_read_only_and_versions_are_compared_numerically(self):
        result = self.run_script(NVIM_VERSION='0.12.5', TREE_VERSION='0.26.10')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.log.exists())
        self.assertNotIn('\x1b', result.stdout)
        result = self.run_script(NVIM_VERSION='0.11.9', TREE_VERSION='0.26.0')
        self.assertEqual(result.returncode, 1)
        self.assertIn('2 prerequisite(s)', result.stdout)
        self.assertFalse(self.log.exists())

    def test_old_rust_is_not_ready(self):
        result = self.run_script(RUST_VERSION='1.84.0')
        self.assertEqual(result.returncode, 1)
        self.assertIn('2 prerequisite(s)', result.stdout)
        self.assertFalse(self.log.exists())

    def test_images_and_unknown_options(self):
        (self.bin / 'magick').unlink()
        self.assertEqual(self.run_script().returncode, 1)
        self.assertEqual(self.run_script('--no-images').returncode, 0)
        self.assertEqual(self.run_script('--bogus').returncode, 2)

    def test_plan_requires_consent_and_rechecks_after_install(self):
        (self.bin / 'git').unlink()
        result = self.run_script('--install')
        self.assertEqual(result.returncode, 1)
        self.assertIn('apt-get install -y git', result.stdout)
        self.assertFalse(self.log.exists())
        result = self.run_script('--install', '--yes')
        self.assertEqual(result.returncode, 1)  # Stub does not actually install git.
        self.assertIn('Setup is incomplete', result.stderr)
        self.assertEqual(self.log.read_text().splitlines(), ['apt-get update', 'apt-get install -y git'])

    def test_install_reaches_ready_and_skips_ready_tools(self):
        (self.bin / 'git').unlink()
        installer = self.bin / 'apt-get'
        installer.write_text(STUB + '\nif [[ $1 == install ]]; then ' +
                             shlex.quote(shutil.which('ln')) +
                             ' -s curl "${0%/*}/git"; fi\n')
        result = self.run_script('--install', '--yes')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Prerequisites ready', result.stdout)
        before = self.log.read_text()
        result = self.run_script('--install', '--yes')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.log.read_text(), before)

    def test_failed_package_update_stops_install_and_cargo(self):
        (self.bin / 'git').unlink()
        (self.bin / 'tree-sitter').unlink()
        result = self.run_script('--install', '--yes', FAIL_INSTALL='1')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.log.read_text().splitlines(), ['apt-get update'])

    def test_brew_python_and_apple_compiler_plan(self):
        (self.bin / 'brew').write_text(STUB)
        (self.bin / 'brew').chmod(0o755)
        (self.bin / 'python3').unlink()
        (self.bin / 'cc').unlink()
        result = self.run_script('--install', TEST_OS='Darwin')
        self.assertEqual(result.returncode, 1)
        self.assertIn('brew install python', result.stdout)
        self.assertIn('xcode-select --install', result.stdout)
        self.assertFalse(self.log.exists())

    def test_dnf_pacman_and_unsupported_system(self):
        (self.bin / 'apt-get').unlink()
        (self.bin / 'cc').unlink()
        (self.bin / 'c++').unlink()
        for manager, plan in [('dnf', 'dnf install -y gcc gcc-c++'),
                              ('pacman', 'pacman -Syu --needed --noconfirm base-devel')]:
            path = self.bin / manager
            path.write_text(STUB)
            path.chmod(0o755)
            result = self.run_script('--install')
            self.assertIn(plan, result.stdout)
            self.assertFalse(self.log.exists())
            path.unlink()
        result = self.run_script('--install', '--yes')
        self.assertEqual(result.returncode, 1)
        self.assertIn('No supported package manager', result.stderr)

    def test_missing_tree_sitter_uses_cargo(self):
        (self.bin / 'tree-sitter').unlink()
        result = self.run_script('--install', '--yes')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.log.read_text().strip(), 'cargo install --locked tree-sitter-cli')


if __name__ == '__main__':
    unittest.main()
