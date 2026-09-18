import json
import os
import random
import struct
from unittest import TestCase
from unittest.mock import Mock, patch

from ipynb_runtime import json_decoder
from ipynb_runtime.json_decoder import NotebookJSONDecoder


class NotebookJSONDecoderTests(TestCase):
    def test_native_decoder_is_called_with_key_cache(self):
        native = Mock()
        native.from_json.return_value = {"value": 7}
        with patch.object(json_decoder, "jiter", native), patch.dict(os.environ, {"IPYNB_DISABLE_RUST": "0"}):
            result = json.loads('{"value": 7}', cls=NotebookJSONDecoder)

        self.assertEqual(result, {"value": 7})
        native.from_json.assert_called_once_with(b'{"value": 7}', cache_mode="keys")

    def test_disable_flag_uses_stdlib(self):
        native = Mock(return_value={"value": 7})
        with patch.object(json_decoder, "jiter", native), patch.dict(
            os.environ, {"IPYNB_DISABLE_RUST": "1"}
        ):
            result = json.loads('{"value": 7}', cls=NotebookJSONDecoder)

        self.assertEqual(result, {"value": 7})
        native.from_json.assert_not_called()

    def test_native_parse_error_falls_back(self):
        native = Mock()
        native.from_json.side_effect = ValueError("unsupported input")
        with patch.object(json_decoder, "jiter", native), patch.dict(os.environ, {"IPYNB_DISABLE_RUST": "0"}):
            result = json.loads('{"value": 2}', cls=NotebookJSONDecoder)

        self.assertEqual(result, {"value": 2})
        native.from_json.assert_called_once()

    def test_stdlib_hooks_are_preserved(self):
        native = Mock(return_value={"value": 7})
        with patch.object(json_decoder, "jiter", native):
            result = json.loads('{"value": 7}', cls=NotebookJSONDecoder, parse_int=str)

        self.assertEqual(result, {"value": "7"})
        native.from_json.assert_not_called()

    def test_notebook_number_and_unicode_semantics_match_stdlib(self):
        text = '{"big": 18446744073709551616, "nan": NaN, "text": "\\ud800"}'

        expected = json.loads(text)
        actual = json.loads(text, cls=NotebookJSONDecoder)

        self.assertEqual(actual["big"], expected["big"])
        self.assertTrue(actual["nan"] != actual["nan"])
        self.assertEqual(actual["text"], expected["text"])

    def test_missing_extension_uses_stdlib(self):
        with patch.object(json_decoder, "jiter", None):
            self.assertEqual(json.loads('{"value": 7}', cls=NotebookJSONDecoder), {"value": 7})

    def test_native_numbers_roundtrip_without_losing_precision(self):
        if json_decoder.jiter is None:
            self.skipTest("optional jiter is not installed")
        rng = random.Random(22)
        values = [2**100, -(2**100), -0.0, float("inf"), float("-inf")]
        values += [struct.unpack("d", rng.randbytes(8))[0] for _ in range(1000)]
        text = json.dumps(values)
        # Directly exercise Rust so a fallback cannot hide a precision change.
        actual = json_decoder.jiter.from_json(text.encode(), cache_mode="keys")
        for expected, parsed in zip(values, actual):
            if expected != expected:
                self.assertNotEqual(parsed, parsed)
            else:
                self.assertEqual(parsed, expected)
                self.assertIs(type(parsed), type(expected))


if __name__ == "__main__":
    import unittest

    unittest.main()
