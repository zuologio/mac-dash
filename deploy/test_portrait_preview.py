"""Exercise preview rollback without touching a real console or systemd."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class PortraitPreviewTests(unittest.TestCase):
    def preview(self, mode):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            rotate = root / 'rotate'
            blank = root / 'blank'
            rotate.write_text('1\n')  # restore the actual old value, not a hardcoded 0
            blank.write_text('4\n')
            original = ['/usr/local/bin/macdash-tui', '--font', '/font with spaces.psf', '--hdmi']
            (root / 'cmdline').write_bytes(('\0'.join(original) + '\0').encode())
            binary = root / 'preview-binary'
            binary.write_text('#!/bin/sh\nexit 0\n')
            binary.chmod(0o755)

            mock = f'#!{sys.executable}\n' + '''
import json, os, pathlib, signal, sys, time
root = pathlib.Path(os.environ['PREVIEW_TEST_ROOT'])
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with (root / 'calls').open('a') as f:
    f.write(json.dumps([name] + args) + '\\n')
if name == 'systemctl' and args[0] == 'show':
    print('123')
if name == 'systemd-run':
    assert (root / 'rotate').read_text().strip() == '3'
    if os.environ['PREVIEW_TEST_MODE'] == 'failure':
        sys.exit(1)
if name == 'stty':
    print('64 75')
if name == 'sleep':
    if os.environ['PREVIEW_TEST_MODE'] == 'signal':
        os.kill(os.getppid(), signal.SIGTERM)
    time.sleep(0.03)
'''
            for name in ['systemctl', 'systemd-run', 'stty', 'sleep']:
                path = root / name
                path.write_text(mock)
                path.chmod(0o755)

            # Redirect only device/process access and the root prerequisite in
            # a disposable copy; execute the real lifecycle/rollback logic.
            source = Path(__file__).with_name('preview-portrait.sh').read_text()
            source = source.replace('[[ $EUID -ne 0 ]]', 'false')
            source = source.replace('/sys/class/graphics/fbcon/rotate', str(rotate))
            source = source.replace('/sys/class/graphics/fb0/blank', str(blank))
            source = source.replace('/proc/$PID/cmdline', str(root / 'cmdline'))
            script = root / 'preview.sh'
            script.write_text(source)
            env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ['PATH'],
                       PREVIEW_TEST_ROOT=tmp, PREVIEW_TEST_MODE=mode)
            result = subprocess.run(['bash', str(script), '10', str(binary)],
                                    env=env, capture_output=True, text=True, timeout=10)
            self.assertEqual(rotate.read_text().strip(), '1', result.stderr)
            self.assertEqual(blank.read_text().strip(), '0', result.stderr)
            calls = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
            self.assertIn(['systemctl', 'start', 'macdash.service'], calls)
            self.assertIn(['systemctl', 'start', 'macdash.timer'], calls)
            self.assertTrue(any(c[:2] == ['systemctl', 'stop'] and 'preview-' in c[-1] for c in calls))
            run = next(c for c in calls if c[0] == 'systemd-run')
            self.assertEqual(run[run.index('--') + 1:], [str(binary.resolve())] + original[1:])
            self.assertIn('restored the previous orientation', result.stdout)
            return result.returncode

    def test_timer_expiry_restores_service_and_orientation(self):
        self.assertEqual(self.preview('success'), 0)

    def test_failed_preview_start_restores_service_and_orientation(self):
        self.assertNotEqual(self.preview('failure'), 0)

    def test_termination_restores_service_and_orientation(self):
        self.assertEqual(self.preview('signal'), 143)


if __name__ == '__main__':
    unittest.main()
