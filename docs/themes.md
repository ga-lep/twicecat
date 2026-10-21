# Custom colour themes

Twicecat ships one colour theme, **Twicecat**. You can add your own as small
JSON files in a `themes` folder, then pick one in **Settings, Appearance,
Theme**. Only colours change: the fonts, the sizes and the layout stay as they
are.

A theme file is treated as untrusted input. It is read under hard limits, off
the window's thread, it may set only the 15 colours listed below, and the app
checks that the result is readable and that the warning, error and success
colours stay apart (rules V1 to V5 below). A colour that is too faint is
adjusted. A theme that cannot be made safe is refused, and Twicecat is used
instead, with a one-line note on the Appearance page.

## Where the folder is

| System | Folder |
|---|---|
| Linux | `$XDG_CONFIG_HOME/twicecat/themes`, or `~/.config/twicecat/themes` when that variable is not set |
| macOS | `~/Library/Application Support/io.github.ga_lep.Twicecat/themes` (next to `settings.json`) |
| Windows | `%APPDATA%\ga_lep\Twicecat\config\themes` |

With `--data-dir <dir>` the themes are read from `<dir>/themes`. With
`--ephemeral` they are read from a folder that is new and empty each time.

The folder does not exist until you ask for it: **Open themes folder** on the
Appearance page creates it (private to you on Linux and macOS) and shows it in
your file manager.

## Try the light example

Save this as `Daylight.light.json`:

```json
{
  "color": {
    "$type": "color",
    "void":            { "$value": "#ffffff" },
    "surface":         { "$value": "#f6f7f9" },
    "surface-raised":  { "$value": "#eceef2" },
    "surface-overlay": { "$value": "#d8dde6" },
    "accent-wash":     { "$value": "#d3e8e3" },
    "line-strong":     { "$value": "#5b6672" },
    "ink":             { "$value": "#14181d" },
    "ink-muted":       { "$value": "#4a5560" },
    "accent":          { "$value": "#0b6b5c" },
    "on-accent":       { "$value": "#ffffff" },
    "accent-bright":   { "$value": "#0b6b5c" },
    "focus":           { "$value": "#0b6b5c" },
    "live":            { "$value": "#b3112f" },
    "ok":              { "$value": "#17622f" },
    "warn":            { "$value": "#8a4b00" }
  }
}
```

Save it in the themes folder:

- Linux: `~/.config/twicecat/themes/Daylight.light.json` (or
  `$XDG_CONFIG_HOME/twicecat/themes/Daylight.light.json` if that variable is
  set);
- macOS: `~/Library/Application Support/io.github.ga_lep.Twicecat/themes/Daylight.light.json`;
- Windows: `%APPDATA%\ga_lep\Twicecat\config\themes\Daylight.light.json`;
- or press **Open themes folder** on the Appearance page and save it there.

Then press **Reload themes** and pick "Daylight" in the Theme menu. The window
turns light. Daylight has one look, light, so nothing else is offered for it.
Pick "Twicecat (default)" to go back.

## File names, one look and pairs

- A file is `<Name>.json`, `<Name>.light.json` or `<Name>.dark.json`. The theme
  is called `<Name>`: 1 to 32 letters, digits, spaces, `_` or `-`, with no space
  at either end. "Twicecat" and "Default" (in any case) name the built-in theme,
  and the names Windows keeps for devices (`CON`, `PRN`, `AUX`, `NUL`, `COM1` to
  `COM9`, `LPT1` to `LPT9`, in any case) are not allowed. Nothing inside the
  file names the theme. Names that differ only in capitals are one name.
- The optional `.light` or `.dark` is a statement the app checks against the
  colours: a file that says light but has dark backgrounds, or the reverse, is
  refused. Without it, the colours decide.
- **A single file is a theme with one look.** It is shown in its own mode,
  light or dark, whatever the system's light/dark setting is. The page shows no
  choice for it.
- **`<Name>.light.json` plus `<Name>.dark.json` make one theme with both
  looks.** For it the page shows "Follow the system", "Light" and "Dark":
  "Follow the system" takes the dark look while the system is dark and the light
  look otherwise (also when the system does not say), and "Light" and "Dark"
  force one look.
- Two files of the same name and the same mode: the first by file name is used
  and the other is reported ("same name and mode as another file").
- If one file of a pair is refused, the other is shown as a theme with one
  look, and the refused file has its own line in the report.

To build a pair, copy `crates/app/assets/themes/Twicecat-copy.json` to
`Nord.dark.json` and the Daylight block above to `Nord.light.json`, edit the
colours, and press **Reload themes**. "Nord" is listed once, with the three
choices under it.

## The 15 tokens

These are the only things a file can set. A missing token takes the built-in
Twicecat value, so a light theme must set its backgrounds and its text.
Values are the built-in ones.

| Token | Meaning | Twicecat |
|---|---|---|
| `void` | the window background and the sidebar | `#0a0812` |
| `surface` | panels | `#110e1c` |
| `surface-raised` | cards, menus and dialogs | `#181428` |
| `surface-overlay` | hovered rows and buttons | `#211b36` |
| `accent-wash` | selected rows and highlighted messages | `#241a45` |
| `line-strong` | borders of fields and checkboxes, dividers | `#6b6194` |
| `ink` | normal text | `#efebfa` |
| `ink-muted` | quiet text, hints, the grey shield | `#a69fc4` |
| `accent` | unread pills, the ring round a person who is talking | `#8e5cff` |
| `on-accent` | text and marks drawn on the accent colours | `#0a0812` |
| `accent-bright` | links, primary buttons, the mention badge | `#b794ff` |
| `focus` | the keyboard focus ring | `#c9aeff` |
| `live` | errors and the red shield | `#ff6b86` |
| `ok` | success | `#5ee0c8` |
| `warn` | warnings | `#ffb547` |

## Start from the built-in colours

Copy `crates/app/assets/themes/Twicecat-copy.json` into the themes folder under
a new name, such as `Mine.dark.json`, and change the colours. It is the
built-in theme under the names above.

Each value is `"#rrggbb"` (or `"#rrggbbaa"` with `ff` as the last two digits),
either as a plain string or as an object with a `hex` string. The format is
that of the design's `tokens.json`; a file written from it still loads, and if
a file has both an older colour name and the new name of the same token, the
new name wins and the report says so. Only the `color` group is read.

## How the colours are used

- The window background and the sidebar are `void`. Panels are `surface`; cards,
  menus and dialogs are `surface-raised`; hovered rows are `surface-overlay`;
  selected rows and highlighted messages are `accent-wash`.
- Text is `ink`, quiet text is `ink-muted`. Links, the info marks and the quote
  bar are `accent-bright`; so are primary buttons and the mention badge. These
  are two departures from the design's own components, on purpose: the design
  fills primary buttons with `accent`, but the app also uses that colour as
  text, which the design allows only for `accent-bright`; and the design shows
  a mention as an unread pill with an "@", but the app tells a mention from an
  unread count by colour.
- The unread pill and the speaking ring are `accent`, with `on-accent` on top.
- Borders are `line-strong`; the focus ring is `focus`.
- Errors, the red shield and the error banner's tint are `live` (the tint is
  `surface-raised` mixed 15 % toward `live`); success is `ok`; warnings are
  `warn`; the grey shield is `ink-muted`.
- The design has no sidebar colour or tinted background, so the sidebar is
  `void` too.

## What the app checks

- **V1, opaque.** A colour that is not a plain opaque `#rrggbb` takes the
  built-in value, and the report counts it.
- **V2, one side.** `void`, `surface`, `surface-raised`, `surface-overlay`,
  `accent-wash` and the error tint must all be dark, or all be light. Otherwise
  the theme is refused ("backgrounds mix light and dark"). The side found is
  the theme's mode.
- **V3, readable.** Text colours (`ink`, `ink-muted`, `accent-bright`, `live`,
  `ok`, `warn`) must reach a contrast of 4.5:1 on every background; `ink` must
  also keep it on hovered and pressed buttons; `on-accent` must reach 4.5:1 on
  both accent colours. `focus`, `accent` and `line-strong` must reach 3:1. A
  colour that is too faint is moved toward white (dark themes) or black (light
  themes) just enough, and the report says how many colours were made more
  readable. If it still cannot be made readable the theme is refused.
- **V4, distinct colours.** `live`, `ok`, `warn` and `accent-bright` must differ
  from each other, and from `ink` and `ink-muted`, clearly (a CIELAB distance of
  at least 20); so must `focus` from the status colours, the text colours and
  `line-strong`, and `accent` from `live` and `ink-muted`. Otherwise the theme
  is refused ("status colours look too alike").
- **V5, visible states.** `accent-wash` must differ from the three backgrounds
  it sits on, and `surface-overlay` from `surface`, `surface-raised` and
  `accent-wash`, enough to show. Otherwise the theme is refused ("selected rows
  would not show").

Limits: at most 1,024 entries of the folder are looked at; at most 32 `.json`
files with allowed names are read, in file-name order ("N more files not
read"); a file is at most 64 KiB; links, pipes and device files are refused; a
leading byte order mark is ignored; nesting is limited. If two keys repeat in a
file, the last wins.

What is not covered: the colours iced itself derives for the radio buttons,
sliders, scrollbars and the selection in a text field are not checked. Look at
them in your theme.

## What is ignored

Everything but the 15 colours: other groups, `components`, `$description` and
the like, unknown token names (counted in the report), aliases such as
`"{color.ink}"`, named colours, `rgb()`, three-digit hex and any colour with
transparency (the token takes the built-in value). Nothing in a file is ever
run, opened as a path or turned into a size.

## Reloading

Themes are read when the app starts and when you press **Reload themes** (no
file watcher). While the folder is being read the button is off and the page
says "Reading the themes folder...". Up to five lines of report follow the
buttons: one per problem file, with a fixed reason and the theme name, never
the file's content.

If the theme you picked is missing or refused, the window shows Twicecat and the
page says why ("Theme "X" could not be used: ... Showing Twicecat."). The name
stays stored, so fixing the file and pressing **Reload themes** applies it.
