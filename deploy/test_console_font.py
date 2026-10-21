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

    def test_rounded_corners_meet_the_fonts_own_lines(self):
        # A font whose ─ is two rows thick at 9-10 and │ one column at 3.
        horizontal = [0] * 16
        horizontal[9] = horizontal[10] = 0xFF
        vertical = [0b00010000] * 16
        lines = font.line_position((horizontal, vertical), 8, 16)
        self.assertEqual(lines, ([9, 10], [3]))
        top_left = font.draw(0x256D, 8, 16, lines)
        bottom_right = font.draw(0x256F, 8, 16, lines)
        # Each corner reaches the cell edges exactly where ─ and │ do.
        self.assertTrue(top_left[9] & 1 and top_left[10] & 1, 'right arm on the line rows')
        self.assertTrue(top_left[15] & 0b00010000, 'down arm on the line column')
        self.assertTrue(bottom_right[9] & 0x80 and bottom_right[10] & 0x80, 'left arm')
        self.assertTrue(bottom_right[0] & 0b00010000, 'up arm')
        # And the corner itself is cut, not square.
        self.assertFalse(top_left[9] & 0b00010000)

    def test_sequences_do_not_replace_single_letters(self):
        self.assertEqual(font.parse_table(b'A\xfefi\xffB\xff', 2, True), {65: 0, 66: 1})
        self.assertEqual(font.parse_table(b'A\0\xfe\xfff\0i\0\xff\xffB\0\xff\xff', 2, False),
                         {65: 0, 66: 1})


if __name__ == '__main__':
    unittest.main()
