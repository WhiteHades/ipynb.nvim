import unittest

import numpy as np
from ipynb_runtime.display import NUMPY_STARTUP


class DisplayTests(unittest.TestCase):
    def test_book_style_preserves_values_and_dtypes(self):
        previous = np.get_printoptions()
        values = [kind(7) for kind in (np.int8, np.int16, np.int32, np.int64,
                                       np.uint8, np.uint16, np.uint32, np.uint64)]
        values += [kind(.5) for kind in (np.float16, np.float32, np.float64)]
        values += [np.complex64(1 + 2j), np.complex128(1 + 2j), np.bool_(True)]
        before = [(value.tobytes(), value.dtype) for value in values]
        try:
            exec(NUMPY_STARTUP, {})
            for value in values:
                self.assertNotIn('np.', repr(value))
            self.assertEqual(before, [(value.tobytes(), value.dtype) for value in values])
            self.assertEqual(repr(np.uint8(7)), '7')
            self.assertEqual(repr(np.float32(.5)), '0.5')
        finally:
            np.set_printoptions(**previous)


if __name__ == '__main__':
    unittest.main()
