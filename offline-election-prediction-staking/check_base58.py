#!/usr/bin/env python3
"""
check_base58_35.py

Reads a file, tokenizes on whitespace or commas, and verifies that every token:
- uses the Bitcoin Base58 alphabet (no 0, O, I, l)
- decodes to exactly 35 bytes

Exit code: 0 if all tokens pass, 1 otherwise.
"""

import argparse
import re
import sys

BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
ALPHABET_INDEX = {c: i for i, c in enumerate(BASE58_ALPHABET)}

def b58decode(s: str) -> bytes:
    """Minimal Base58 (Bitcoin alphabet) decoder with leading-zero handling."""
    # Validate characters
    for ch in s:
        if ch not in ALPHABET_INDEX:
            raise ValueError(f"invalid Base58 char: {ch!r}")

    # Convert Base58 string to integer
    num = 0
    for ch in s:
        num = num * 58 + ALPHABET_INDEX[ch]

    # Handle leading '1' → leading zero bytes
    leading_zeros = len(s) - len(s.lstrip("1"))

    # Convert integer to bytes (big-endian)
    payload = b"" if num == 0 else num.to_bytes((num.bit_length() + 7) // 8, "big")
    return b"\x00" * leading_zeros + payload

def is_base58_35_bytes(s: str):
    """Return (ok: bool, reason: str | None)."""
    s = s.strip()
    if not s:
        return False, "empty token"
    try:
        decoded = b58decode(s)
    except ValueError as e:
        return False, str(e)
    if len(decoded) != 35:
        return False, f"decoded length {len(decoded)} != 35"
    return True, None

def main():
    ap = argparse.ArgumentParser(description="Check that tokens are Base58 and decode to exactly 35 bytes.")
    ap.add_argument("path", help="Path to input file. Tokens are split on whitespace or commas.")
    args = ap.parse_args()

    try:
        data = open(args.path, "r", encoding="utf-8").read()
    except OSError as e:
        print(f"Error reading file: {e}", file=sys.stderr)
        sys.exit(1)

    # Tokenize on whitespace or commas; skip blanks and comment lines starting with '#'
    tokens = []
    for line in data.splitlines():
        line = line.strip()
        line = line[1:-2]
        if not line or line.startswith("#"):
            continue
        tokens.extend([t for t in re.split(r"[\s,]+", line) if t])

    if not tokens:
        print("No tokens found.")
        sys.exit(1)

    all_ok = True
    for i, tok in enumerate(tokens, 1):
        ok, reason = is_base58_35_bytes(tok)
        if ok:
            print(f"[OK]  #{i}: {tok}")
        else:
            all_ok = False
            print(f"[BAD] #{i}: {tok}  →  {reason}")

    if all_ok:
        print(f"\nAll {len(tokens)} tokens are valid Base58 and decode to 35 bytes.")
        sys.exit(0)
    else:
        print(f"\n{sum(1 for t in tokens if is_base58_35_bytes(t)[0])}/{len(tokens)} tokens passed.")
        sys.exit(1)

if __name__ == "__main__":
    main()
