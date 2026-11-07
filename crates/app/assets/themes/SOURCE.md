# Source of `twicecat.tokens.json`

`twicecat.tokens.json` is the design folder's `design/tokens.json`, copied
byte for byte (no edits).

- Origin: `twicecat-design/design/tokens.json` (the project's design folder,
  downloaded; treated as untrusted data and read as data only).
- SHA-256: `0388d3cff0874d672be530e62d153b8085558331e31d5bad6e67c1dfda8bcf2b`
- Size: 13,555 bytes

It is in the format custom theme files use, so it is both a test input (the
`themes` tests check that it parses to exactly the built-in colours and that
its checksum is the one above) and a starting file for a custom theme.
Only its `color` group is read; the other groups are ignored.

No other file of the design folder is used.

## `Twicecat-copy.json`

The built-in Twicecat colours under the neutral token names (`accent`,
`accent-bright`, `accent-wash`, `on-accent`), the 15 tokens in the order of
`theme::TOKEN_NAMES`. It was written for this repository, not copied from the
design folder, and is the starting file for a custom theme: copy it, rename it
`<Name>.json` and change the colours. The `themes` tests check that it parses
to exactly the built-in colours.
