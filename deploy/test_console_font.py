"""Regression checks for the console font patcher: python3 -m unittest discover -s deploy."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('console_font', Path(__file__).with_name('make-console-font.py'))
font = importlib.util.module_from_spec(spec)
spec.loader.exec_module(font)


class ConsoleFontTests(unittest.TestCase):
    def test_patch_preserves_typeface_and_can_be_repeated(self):
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / 'font.psf')
            original = font.Font(True, 1, 16, 8, 16,
                                 [bytearray([i % 256] * 16) for i in range(256)],
                                 {cp: cp for cp in range(32, 127)})
            font.write(original, path)
            font.build(path, path)
            patched = font.read(path)
            self.assertTrue(all(cp in patched.mapping for cp in font.NEEDED))
            self.assertTrue(all(any(patched.glyphs[patched.mapping[cp]]) for cp in font.NEEDED))
            for cp in range(32, 127):
                self.assertEqual(patched.glyphs[patched.mapping[cp]], original.glyphs[cp])
            first = Path(path).read_bytes()
            font.build(path, path)
            self.assertEqual(Path(path).read_bytes(), first)

    def test_sequences_do_not_replace_single_letters(self):
        self.assertEqual(font.parse_table(b'A\xfefi\xffB\xff', 2, True), {65: 0, 66: 1})
        self.assertEqual(font.parse_table(b'A\0\xfe\xfff\0i\0\xff\xffB\0\xff\xff', 2, False),
                         {65: 0, 66: 1})


if __name__ == '__main__':
    unittest.main()
