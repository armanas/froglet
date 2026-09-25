"""RFC 8785 (JCS) canonicalization tests.

Covers: the RFC 8785 example from ``canonical_json.rs``'s own test
(``canonicalizes_rfc8785_example``), float rejection, and bool-vs-int
handling (Python's ``bool`` being an ``int`` subclass is the main trap this
module has to avoid).
"""

from __future__ import annotations

import unittest

from froglet_verify.jcs import canonicalize


class RFC8785ExampleTests(unittest.TestCase):
    def test_rfc8785_example_int_analog(self) -> None:
        # canonical_json.rs's `canonicalizes_rfc8785_example` test encodes
        # `{"b": false, "c": 12e1, "a": "Hello!"}` (note `c` is the float
        # 12e1 = 120.0) to `{"a":"Hello!","b":false,"c":120}`. This module
        # deliberately fails closed on floats (RFC 8785's ECMAScript
        # Number::toString formatting is out of scope -- see jcs.py's module
        # docstring), so the same expected output string is exercised here
        # via the equivalent plain int 120 in `c`'s place; float rejection
        # itself is covered separately below.
        value = {"b": False, "c": 120, "a": "Hello!"}
        self.assertEqual(canonicalize(value), b'{"a":"Hello!","b":false,"c":120}')

    def test_nested_object_key_order_does_not_change_output(self) -> None:
        # Mirrors canonical_json.rs's `nested_object_key_order_does_not_change_output`.
        first = {"outer": {"b": 2, "a": 1}}
        second = {"outer": {"a": 1, "b": 2}}
        self.assertEqual(canonicalize(first), canonicalize(second))

    def test_top_level_key_order_does_not_change_output(self) -> None:
        self.assertEqual(canonicalize({"b": 2, "a": 1}), canonicalize({"a": 1, "b": 2}))


class FloatRejectionTests(unittest.TestCase):
    def test_bare_float_rejected(self) -> None:
        with self.assertRaises(ValueError):
            canonicalize(1.5)

    def test_whole_number_float_rejected(self) -> None:
        # Even a float whose value is mathematically an integer (like the
        # RFC 8785 example's 12e1) must still raise -- this module never
        # attempts to detect/reformat "integer-valued floats".
        with self.assertRaises(ValueError):
            canonicalize(120.0)

    def test_float_inside_object_rejected(self) -> None:
        with self.assertRaises(ValueError):
            canonicalize({"a": 12e1})

    def test_float_inside_nested_array_rejected(self) -> None:
        with self.assertRaises(ValueError):
            canonicalize({"a": [1, 2, {"b": 3.0}]})

    def test_nan_rejected(self) -> None:
        with self.assertRaises(ValueError):
            canonicalize(float("nan"))

    def test_infinity_rejected(self) -> None:
        with self.assertRaises(ValueError):
            canonicalize(float("inf"))
        with self.assertRaises(ValueError):
            canonicalize(float("-inf"))


class BoolVsIntTests(unittest.TestCase):
    def test_bool_serializes_as_literal_not_as_int(self) -> None:
        self.assertEqual(canonicalize(True), b"true")
        self.assertEqual(canonicalize(False), b"false")

    def test_bool_and_int_are_not_interchangeable(self) -> None:
        # isinstance(True, int) is True in Python -- naive code that checks
        # `isinstance(value, int)` before `isinstance(value, bool)` would
        # collapse these to the same encoding. They must differ.
        self.assertNotEqual(canonicalize({"a": True}), canonicalize({"a": 1}))
        self.assertNotEqual(canonicalize({"a": False}), canonicalize({"a": 0}))

    def test_bool_in_list_preserves_type_and_position(self) -> None:
        self.assertEqual(canonicalize([True, False, 1, 0]), b"[true,false,1,0]")

    def test_bool_as_dict_key_value_pair_round_trips_distinctly(self) -> None:
        self.assertEqual(canonicalize({"ok": True}), b'{"ok":true}')
        self.assertEqual(canonicalize({"ok": 1}), b'{"ok":1}')


class MiscellaneousEncodingTests(unittest.TestCase):
    def test_none_serializes_as_null(self) -> None:
        self.assertEqual(canonicalize(None), b"null")

    def test_negative_integer(self) -> None:
        self.assertEqual(canonicalize(-42), b"-42")

    def test_zero(self) -> None:
        self.assertEqual(canonicalize(0), b"0")

    def test_large_integer_no_precision_loss(self) -> None:
        # u64::MAX-scale values must round-trip exactly (unlike IEEE-754
        # doubles / JS numbers); Python ints are arbitrary precision.
        big = 2**64 - 1
        self.assertEqual(canonicalize(big), str(big).encode("ascii"))

    def test_string_escaping_matches_json_spec(self) -> None:
        self.assertEqual(canonicalize('a"b\\c\nd'), b'"a\\"b\\\\c\\nd"')

    def test_non_ascii_strings_are_raw_utf8_not_u_escaped(self) -> None:
        encoded = canonicalize("café")
        self.assertEqual(encoded, '"café"'.encode("utf-8"))
        self.assertNotIn(b"\\u", encoded)

    def test_separators_have_no_whitespace(self) -> None:
        encoded = canonicalize({"a": 1, "b": [1, 2]})
        self.assertNotIn(b" ", encoded)
        self.assertEqual(encoded, b'{"a":1,"b":[1,2]}')

    def test_empty_object_and_array(self) -> None:
        self.assertEqual(canonicalize({}), b"{}")
        self.assertEqual(canonicalize([]), b"[]")

    def test_non_string_dict_key_rejected(self) -> None:
        with self.assertRaises(TypeError):
            canonicalize({1: "a"})  # type: ignore[dict-item]

    def test_unsupported_type_rejected(self) -> None:
        with self.assertRaises(TypeError):
            canonicalize(object())


class Utf16KeyOrderingTests(unittest.TestCase):
    def test_keys_sort_by_utf16_code_unit_not_code_point(self) -> None:
        # U+10000 (astral; UTF-16 surrogate pair D800 DC00) has a *larger*
        # Unicode code point than U+E000 (BMP private-use area; single code
        # unit E000), so naive Python string comparison (which compares by
        # code point) would sort U+E000 first. But RFC 8785 requires
        # sorting by UTF-16 *code unit sequence*, under which the astral
        # character's leading surrogate (0xD800) is less than 0xE000, so it
        # must sort *first*. This is the one case where UTF-16 code unit
        # order and Unicode code point order disagree, and canonicalize()
        # must follow the former.
        astral = "\U00010000"
        bmp_private_use = ""
        self.assertGreater(
            astral, bmp_private_use
        )  # code point order: astral is greater

        encoded = canonicalize({bmp_private_use: "second", astral: "first"})
        expected = (
            '{"' + astral + '":"first","' + bmp_private_use + '":"second"}'
        ).encode("utf-8")
        self.assertEqual(encoded, expected)


if __name__ == "__main__":
    unittest.main(verbosity=2)
