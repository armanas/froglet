"""RFC 8785 (JSON Canonicalization Scheme) for the Froglet kernel's JSON subset.

Froglet signs artifacts by hashing/serializing payloads through the JCS
canonicalization defined in RFC 8785, implemented on the Rust side by the
``serde_json_canonicalizer`` crate (see ``froglet-protocol/src/canonical_json.rs``).

Froglet kernel payloads only ever contain a restricted subset of JSON value
types: strings, integers, booleans, arrays, objects, and null. This module
implements exactly that subset:

- Object keys are sorted by their UTF-16 code unit sequence (RFC 8785
  section 3.2.3), not by Python's native code-point ordering. For the
  Basic Multilingual Plane the two orderings coincide; they can differ for
  astral characters (which UTF-16 represents as surrogate pairs), so keys
  are sorted by their big-endian UTF-16 encoding rather than by raw string
  comparison, to match RFC 8785 exactly even in that edge case.
- Separators are the RFC 8785-mandated ``','`` and ``':'`` with no
  whitespace.
- Strings are emitted as raw UTF-8 (``ensure_ascii=False``); only the
  characters JSON requires escaping (quote, backslash, and control
  characters) are escaped.
- Integers are emitted as plain decimal text with no exponent, matching how
  Rust's ``serde_json`` serializes integer-typed numbers.

Floating point numbers are deliberately **not** supported. RFC 8785 requires
formatting floats via the ECMAScript ``Number::toString`` algorithm (shortest
round-trip decimal, with its own exponent/precision rules), which is a
substantial undertaking to replicate exactly and is unnecessary here: every
numeric field in the Froglet kernel schema (fees in millisatoshis, byte/time
limits, timestamps, fuel limits) is an integer. Passing a Python ``float``
raises ``ValueError`` instead of silently producing output that might not
byte-for-byte match ``serde_json_canonicalizer`` — fail closed rather than
approximate.

Note that in Python ``bool`` is a subclass of ``int`` (``isinstance(True,
int)`` is ``True``), so this module checks for ``bool`` before ``int`` to
avoid serializing ``True``/``False`` as ``1``/``0``.
"""

from __future__ import annotations

import json
from typing import Any

__all__ = ["canonicalize"]


def _encode_json_string(value: str) -> str:
    # `json.dumps` with `ensure_ascii=False` escapes exactly the characters
    # RFC 8259 (and thus RFC 8785) requires escaping -- '"', '\\', and control
    # characters via the short named escapes ('\b' '\f' '\n' '\r' '\t') or
    # '\u00XX' for the remaining control codes -- and leaves every other
    # Unicode code point as raw UTF-8. That matches serde_json's `Serialize`
    # for `str`, so this is reused verbatim rather than hand-rolled.
    return json.dumps(value, ensure_ascii=False)


def _utf16_sort_key(key: str) -> bytes:
    # Sorting by the big-endian UTF-16 code unit encoding (with surrogate
    # pairs for astral characters) reproduces RFC 8785's "UTF-16 code unit"
    # key ordering under ordinary lexicographic byte comparison, including
    # for characters outside the Basic Multilingual Plane where UTF-16 code
    # unit order and Unicode code point order can disagree.
    return key.encode("utf-16-be", "surrogatepass")


def _write(value: Any, out: list[str]) -> None:
    if value is None:
        out.append("null")
    elif isinstance(value, bool):
        # Must be checked before `int`: bool is an int subclass in Python.
        out.append("true" if value else "false")
    elif isinstance(value, int):
        out.append(str(value))
    elif isinstance(value, float):
        raise ValueError(
            "froglet_verify.jcs does not support floating point numbers "
            f"(JCS subset is integers only): {value!r}"
        )
    elif isinstance(value, str):
        out.append(_encode_json_string(value))
    elif isinstance(value, (list, tuple)):
        out.append("[")
        for index, item in enumerate(value):
            if index:
                out.append(",")
            _write(item, out)
        out.append("]")
    elif isinstance(value, dict):
        out.append("{")
        for key in value.keys():
            if not isinstance(key, str):
                raise TypeError(f"JCS object keys must be strings, got {type(key)!r}")
        for index, key in enumerate(sorted(value.keys(), key=_utf16_sort_key)):
            if index:
                out.append(",")
            out.append(_encode_json_string(key))
            out.append(":")
            _write(value[key], out)
        out.append("}")
    else:
        raise TypeError(f"unsupported type for JCS canonicalization: {type(value)!r}")


def canonicalize(obj: Any) -> bytes:
    """Serialize ``obj`` to its RFC 8785 canonical JSON encoding.

    Mirrors ``froglet_protocol::canonical_json::to_vec`` (a thin wrapper
    around the ``serde_json_canonicalizer`` crate) for the Froglet kernel's
    restricted JSON subset (str, int, bool, list, dict, None).

    Raises:
        ValueError: ``obj`` contains a ``float`` (including NaN/Infinity).
        TypeError: ``obj`` contains a value of an unsupported type, or a
            ``dict`` with a non-string key.

    Example:
        >>> canonicalize({"b": False, "c": 120, "a": "Hello!"})
        b'{"a":"Hello!","b":false,"c":120}'
    """
    out: list[str] = []
    _write(obj, out)
    return "".join(out).encode("utf-8")
