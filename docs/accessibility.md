# Accessibility

What Twicecat does for keyboard use, contrast, zoom and assistive technology
after the UI/UX rework, and what it does not do yet. Checked against iced
0.14.0 (pinned). The sections are: the keyboard map, Settings and Sessions,
Calls, Screen readers, Drag and drop on Wayland, Colour sets and contrast,
Zoom, Icons and tooltips, Narrow windows, How calls show state, Room dialogs,
and the Known limits of iced 0.14.

## Keyboard map

One table, `SHORTCUTS` in `crates/app/src/keys.rs`, drives both the key
handling and the list in Settings, so they cannot drift apart. Ctrl reads Cmd
on macOS.

| Keys | Action |
|------|--------|
| Ctrl+K | Switch room (opens the room switcher) |
| Alt+Down / Alt+Up | Open the next / previous room in the list |
| Ctrl+Shift+Down / Ctrl+Shift+Up | Open the next / previous room with unread messages |
| Ctrl+B | Collapse the room list to a rail, or expand it again (saved) |
| Ctrl+U | Attach a file to the open room |
| Ctrl+, | Open Settings (always on the Profile page) |
| Ctrl+= / Ctrl+- / Ctrl+0 | Make the text larger / smaller / back to 100 % |
| Ctrl+Shift+M | In a call: mute or unmute the microphone |
| Ctrl+Shift+D | In a call: deafen or undeafen |
| Ctrl+Shift+O | In a call: turn the camera on or off |
| Ctrl+Shift+E | In a call: share your screen, or stop sharing |
| Ctrl+Shift+A / Ctrl+Shift+X | While a call rings (its card is showing): accept / decline it |
| Tab / Shift+Tab | Move to the next / previous field |
| Esc | Close the topmost dialog, panel, Settings, the Sessions card or the switcher |
| Up / Down (switcher open) | Highlight the previous / next room |
| Enter (switcher open) | Open the highlighted room |
| Enter / Shift+Enter (composer) | Send / new line |

Rules the code follows (all in tests):

- The composer keeps its own keys. Enter, Shift+Enter, Up (edit the last
  message, or move the cursor) and Tab (pick a suggestion) are never taken by
  a shortcut. The chords above are taken once: by the composer's key binding
  when it has the focus, otherwise by the window listener.
- Up and Down move the switcher highlight only while the switcher is open.
- Tab moves the focus only when no widget used the key and no suggestion list
  is open. The order is the widget tree's order; overlays are layers in a
  fixed stack of two, so opening one never reshuffles the base.
- Esc closes one layer per press, topmost first, in the order the layers are
  drawn (tests open several and check that each press closes the one on top):
  1. the incoming-call card (hidden, and its ringtone silenced; the room keeps
     ringing);
  2. the verification card (it only cancels or closes, never confirms);
  3. the switcher;
  4. the upload confirmation;
  5. the viewer;
  6. the Voice & audio card;
  7. Settings;
  8. a room dialog, the user card, the room settings (a question about leaving
     or encryption first) or the member list;
  9. the share picker (it only cancels, never starts a share), then the
     full-screen view of a video;
  10. a security screen (recovery setup, a question about changing the key or
     resetting the identity);
  11. the logout warning, then the Sessions card's password prompt, its
      sign-out question, a rename in progress, then the card itself;
  12. the composer's suggestions, then its reply or edit mode;
  13. the thread panel, then the participants panel of a call (the one on
      screen first when the window is too narrow for both).

  A text field that took the Esc to drop its focus does not keep an overlay
  open. Esc never confirms anything. Held-down Esc repeats do not decline a
  queued verification request.
- Closing the switcher with Esc gives the focus back to the composer.
- Ctrl+K and the room-stepping keys do nothing while another overlay covers
  the main view.
- Ctrl+B works on the main screen only, and also over a dialog. The choice is
  the **Room list** setting, so it is saved. The rail keeps every room-list
  key: Alt+Down / Alt+Up and Ctrl+Shift+Down / Ctrl+Shift+Up step through the
  same list, and the open room stays marked by a fill and a 3 px bar. Its
  avatars are reached by the pointer and by those room-stepping keys and the
  Ctrl+K switcher; Tab does not visit them, since iced 0.14 buttons take no
  keyboard focus. Each has a tooltip with the room's name and what the dots
  mean (unread, mentions, in a call, ringing, muted), and a dot is never the only sign, since the tooltip words
  say the same. In the rail the search field is a **Search** button that
  expands the list, and the call dock is a column of icon buttons (the call's
  room, mute, deafen, leave), each with its words in a tooltip, and visible
  marks that need no pointing: the call phase glyph, the encryption lock (or a
  warning when the call is not encrypted), who declined, and the microphone's
  state (see Calls).
- Narrow windows: from 800 to 1100 px the list is shown as the rail while a
  thread or the participants panel is open; under 800 px the spaces sidebar is
  not shown, the list is the rail, and an open panel takes the room pane's
  place (the room pane stays, with no width, and takes no keys). Only one panel
  shows then: the one opened last; Esc closes the one on screen first and then
  returns to the room. The saved room list choice is not changed by the width.
  While a panel fills the window its header repeats the trust tag of a direct
  message, and the room header stacks its buttons under the name in a narrow
  pane so nothing is cut off.
- The ring chords Ctrl+Shift+A and Ctrl+Shift+X do nothing while a focused
  text field captures the keys: a text field takes Ctrl+Shift+A and
  Ctrl+Shift+X itself (select all, cut), so move the focus out of the field
  (click elsewhere or press Esc in it) first. The composer forwards them when
  it has the focus; other fields (the display name, search, the server field)
  do not. They also do nothing when no card is showing, during the first
  0.6 s of a card, and when the key is still held from the previous ring.
- Ctrl+Shift+M, Ctrl+Shift+D, Ctrl+Shift+O and Ctrl+Shift+E act only during
  a call, also over a dialog (the call goes on behind it); without a call they
  do nothing. Ctrl+Shift+V is not used for the camera: in the composer it is
  paste.
- While the Voice & audio card waits for a push-to-talk key (**Set key…**),
  every key goes to it and no shortcut runs: the next key with its
  modifiers becomes the binding, a modifier alone keeps waiting, and Esc,
  **Cancel**, closing the card or leaving the window stop waiting. A mouse
  button other than left and right pressed then becomes the mouse binding
  instead; left and right still work the card.

## Settings and Sessions

Settings is a card with a list of pages at its left (Profile, Appearance,
Notifications, Voice & audio, Video, Keyboard, Sessions, About) and one page
at its right. Ctrl+, always opens it on Profile, and so does opening it again
after a close.

- Two levels. **Show advanced settings** (a checkbox in the card's header,
  off by default and remembered) adds, under a rule on the same pages, what
  a person with little technical knowledge does not need: density and the
  room list (Appearance), keywords, message content in notifications and the
  tray badge (Notifications), a button to the Voice & audio card with every
  audio option, and on Video every option the camera and screen sharing can
  be tuned by. A simple page never shows a control that only does something
  at the advanced level, and every setting an advanced person changed stays
  in force when the switch goes back off. The simple Voice & audio page
  holds the microphone, the speakers, the volume, how you talk (and the key),
  noise reduction in three words (Off, Standard, Strong) and a test with its
  level; the call's audio buttons open it, or the card at the advanced level.
  The simple Video page holds the camera with a preview, and a picture
  quality in words for the camera and for sharing.
- Esc closes Settings, as before (one layer per press).
- Tab and Shift+Tab walk the text fields of the page on show, in the order of
  the widget tree: the display name on Profile, the keyword field on
  Notifications at the advanced level, and none named on the other pages (the
  advanced Video page has fields for custom numbers). This iced gives only text
  fields the keyboard focus; buttons, checkboxes, radio buttons and the list
  of pages never take it, so they are reached with the pointer. This is a
  limit of the pinned iced, not a choice; it is on the human checkpoint list.
  A page change keeps the text typed in the fields of the other pages.
- The Sessions page of Settings has one button, **Manage sessions...**, which
  closes Settings and opens the Sessions card. That card closes with its
  **Close** button or with Esc. Esc closes one thing per press, topmost
  first: the logout warning, the password prompt, the sign-out question, then
  the card. It only ever cancels or closes, never confirms a sign-out or a
  log-out, and while the card or one of its prompts is open it never reaches
  the room behind it (reply or edit, thread panel, call panel). While the
  password is on its way to the server the prompt cannot be cancelled, and
  Esc does nothing. A session named like a tag ("Verified", "This device") or
  like a device ID is shown in quotes. The rename field and the password prompt
  are text fields; the prompt takes the focus when it opens.
- The destructive buttons of Sessions (Sign out, Sign out of all other
  sessions, Sign out anyway) are red, say what they do in words, sit at least
  12 px from every other control, and each asks first: the button that asks is
  not where the one that confirms appears. No key presses a button, so Enter
  cannot confirm one; Enter in the password field sends it only when the field
  is not empty.
- Whether a session is verified is shown in words (**This device**,
  **Verified**, **Not verified**, **No encryption keys**) and in colour.
  Session names and IDs come from the server: they are cleaned, capped and
  wrapped inside the row, and the end of an ID is never cut off.

## Settings and Sessions

Settings is a card with a list of pages at its left (Profile, Appearance,
Notifications, Voice & audio, Keyboard, Sessions, About) and one page at its
right. Ctrl+, always opens it on Profile, and so does opening it again after
a close.

- Esc closes Settings, as before (one layer per press).
- Tab and Shift+Tab walk the text fields of the page on show, in the order of
  the widget tree: the display name on Profile, the keyword field on
  Notifications, and no field on the other pages. This iced gives only text
  fields the keyboard focus; buttons, checkboxes, radio buttons and the list
  of pages never take it, so they are reached with the pointer. This is a
  limit of the pinned iced, not a choice; it is on the human checkpoint list.
  A page change keeps the text typed in the fields of the other pages.
- The Sessions page of Settings has one button, **Manage sessions...**, which
  closes Settings and opens the Sessions card. That card closes with its
  **Close** button or with Esc. Esc closes one thing per press, topmost
  first: the logout warning, the password prompt, the sign-out question, then
  the card. It only ever cancels or closes, never confirms a sign-out or a
  log-out, and while the card or one of its prompts is open it never reaches
  the room behind it (reply or edit, thread panel, call panel). While the
  password is on its way to the server the prompt cannot be cancelled, and
  Esc does nothing. A session named like a tag ("Verified", "This device") or
  like a device ID is shown in quotes. The rename field and the password prompt
  are text fields; the prompt takes the focus when it opens.
- The destructive buttons of Sessions (Sign out, Sign out of all other
  sessions, Sign out anyway) are red, say what they do in words, sit at least
  12 px from every other control, and each asks first: the button that asks is
  not where the one that confirms appears. No key presses a button, so Enter
  cannot confirm one; Enter in the password field sends it only when the field
  is not empty.
- Whether a session is verified is shown in words (**This device**,
  **Verified**, **Not verified**, **No encryption keys**) and in colour.
  Session names and IDs come from the server: they are cleaned, capped and
  wrapped inside the row, and the end of an ID is never cut off.

## Calls

Every call control is a button, a menu or a slider in the widget tree's
order, so Tab reaches it:

- the room header's call button (Call, Join call, Join voice, Leave call)
  and, in the call's room, **People** for the participants panel;
- the call dock at the bottom of the room list: the call's room name (opens
  it with the panel), **Mute**, **Deafen**, **Audio settings**, **Leave**
  (off while a join or leave waits), and the close button of a call notice
  (a separate button: clicking the notice's mark or words does nothing);
- per person in the participants panel: **More**, which folds out **−** and
  **+** (10 % each), the volume slider, and the voice cleaning menu;
- the Voice & audio card (from Settings or the call bar's **Audio…**): menus,
  checkboxes, and for each slider **−** and **+** beside it; Esc closes it.
  Its Talking section sets voice activity, push-to-talk or toggle-to-talk,
  the key (**Set key…**, **Clear**), the mouse button and the gamepad
  button (menus, so no mouse button or pad is needed to pick one) and the
  release delay;
- the incoming-call card, a modal layer above everything else: **Decline**
  then **Accept**, side by side with a gap, and Esc to hide it. iced 0.14's
  buttons take no keys, so the card also answers to **Ctrl+Shift+A** (accept)
  and **Ctrl+Shift+X** (decline), also with the composer focused, only while
  the card is showing. For about 0.6 s after the card shows (or changes to
  another ring) the buttons are off, with the words "Answer in a moment", and
  the keys do nothing; a chord that is still held never answers a second
  ring: release it and press it again. While it shows, the room shortcuts wait, as under any
  dialog. The caller's name or user ID is cleaned, cut in the middle past 64
  characters (the end, the server name, stays) and wraps by glyph, so no
  character is hidden.

Push-to-talk works from the keyboard alone: the bound key in Twicecat's
window always talks, also with the composer focused (a key that types text
also types it, so a function key or a chord is the better choice), and the
window losing focus lets go. The call dock says "Talking", or which key to
hold or press, in words, and draws the microphone so that it can be read
without pointing at it: an open microphone (voice activity, or transmitting)
is the microphone glyph, muted or deafened is the crossed microphone (and
takes precedence over everything), and push-to-talk or toggle-to-talk that
is not transmitting is the key (or gamepad) glyph on an outlined fill, so
"muted" and "waiting for the key" are never the same picture. Transmitting
has the accent fill.

A ring is not only a sound: the card, the window asking for attention
(until it is focused, for a direct call), a desktop notification while the
window is not focused, and the room's highlighted call pill.

iced 0.14's slider takes no keyboard focus: its arrow keys work only while
the mouse is over it (`iced_widget` 0.14.2 `src/slider.rs:393-405`), which
is why **−** and **+** sit beside every slider (the people's volumes, the
microphone gain, the volume, the cleaning limits, the voice level, the
release delay). A slider moved by the wheel or the
keys, which never reports a release, sends its value after a 400 ms pause.
Speaking is shown by a ring around the picture and by the word "Speaking",
muted by a mark and the word "Muted", so neither relies on colour alone.

### Video in calls

- The camera and the share are buttons in the call dock (and the rail's
  column), and in the stage's header: **Camera** and **Share your screen**.
  Each shows that it is on by colour and by a check beside the glyph, and
  its tooltip says what a press does. They answer to Ctrl+Shift+O and
  Ctrl+Shift+E.
- The stage is above the timeline of the call's room while someone sends a
  picture. It has a **Hide** button (the pictures stop being sent to this
  window while it is hidden), and a line with **Show video** when it is.
  Each tile writes the person's name, "(you)" for the own camera, "screen" for a
  share, a mark and the words "Camera off" when the camera is off, and the
  microphone mark when they are muted. Speaking is a thick ring in the
  speaking colour around the tile, as for the avatars. A person without a
  camera shows their avatar. **Pin** keeps a tile in the big place;
  **Full screen** opens it over the window (Esc, or **Exit full screen**).
- The share picker lists the screens and windows with a picture, a name and
  the size in words, or says that the system will ask. **Share** stays off
  until something is picked. Esc cancels it.
- While a share goes out, **You are sharing your screen** with **Stop sharing**
  floats at the bottom of the window over every dialog and card.
- Pictures are drawn by the GPU renderer. When only the software renderer
  runs, the stage says so in words instead of staying black.
- A camera that is unplugged while it is on (in a call or in the Video
  page's preview) turns off, and a notice says "the camera was
  disconnected" in words; the preview does not stay frozen on its last
  picture. The camera list on the Video page follows cameras as they are
  plugged in and out.

## Screen readers: not supported

Twicecat cannot be used with a screen reader. iced 0.14 has no accessibility
tree: there is no AccessKit integration in `iced`, `iced_core`, `iced_winit`
or `iced_widget` (a search of the pinned sources for "accesskit" finds
nothing), so no widget exposes a role, a name or a state to AT-SPI, and the
window shows up as one unnamed surface.

What to watch: the iced project's accessibility work (AccessKit support).
Re-check at every iced upgrade (search the new sources for `accesskit`, read
the release notes) and record the result in `PROGRESS.md`. Until it lands the
keyboard map and the contrast rules below are all Twicecat offers.

## Drag and drop on Wayland

Dragging files into the window does not work on a native Wayland window: the
iced 0.14 / winit stack delivers no file-drop events there (works on X11 and
XWayland). Use **Attach...** or Ctrl+U, or paste with Ctrl+V. Settings shows
the same note under the shortcut list.

## Colour sets and contrast

- There are four colour sets ("families"): Twicecat, Frost, Ember and High
  contrast, each with a light and a dark palette (`theme::Family`). Settings,
  Appearance picks the family (four swatches, the one in use is outlined and
  has a check) and the mode (System, Light or Dark). **Use the default** goes
  back to the app's default set (`theme::DEFAULT_FAMILY`), which is **Frost**
  (the swatch marked "(default)"). People who never picked a set see Frost,
  including existing users after upgrading from a version whose default was
  Twicecat; anyone who picked a set, Twicecat included, keeps it.
- Text: all eight palettes are tested for a contrast of at least **4.5:1**
  (WCAG AA) for text on background, muted text, badge and highlight text,
  text on selection, message highlight, hover, surface, raised surface and
  sidebar, the error text on its soft background, and the primary and focus
  colours on the background. The tightest pair per palette, as the test
  computes them:

  | Family | Light | Dark |
  |--------|-------|------|
  | Twicecat | 4.67:1 (error text on its soft background) | 4.87:1 (muted text on a highlighted message) |
  | Frost (default) | 4.69:1 (muted text on the sidebar) | 5.22:1 (muted text on a highlighted message) |
  | Ember | 4.68:1 (muted text on the sidebar) | 4.64:1 (muted text on a highlighted message) |
  | High contrast | 7.70:1 (muted text on the sidebar) | 7.07:1 (error text on its soft background) |

- **High contrast** (black on white, white on black, saturated accents) is
  tested for **7:1** (WCAG AAA) on all of those pairs. Choose it in Settings,
  Appearance, Colours, with Light or Dark mode.
- Non-text: the ring around a person who is talking (`speaking`) reaches
  **3:1** against the background in all eight palettes (WCAG 1.4.11). The
  tested values are 5.08 (Twicecat light), 6.49 (Twicecat dark), 5.04 (Frost
  light), 9.57 (Frost dark), 5.02 (Ember light), 8.39 (Ember dark), 7.44
  (High contrast light) and 16.04 (High contrast dark). Panel borders only
  have to differ from the background; the words and marks next to them carry
  the meaning.
- Sender names take a colour derived from the user ID and are lightened or
  darkened until they reach 4.5:1 on the current background, in all eight.
- Not tested by code (on the human checklist): that the keyboard focus is
  visible on every control in both themes, since it depends on iced's widget
  styles, and how each set looks in a real window.

## Zoom

**Zoom** scales the whole window from 80 % to 200 % in steps of 10 %: Ctrl+=
makes it larger, Ctrl+- smaller and Ctrl+0 resets to 100 % (Cmd on macOS).
Settings, Appearance has the same as **Smaller**, **Larger** and **Reset**
buttons. The keys are ignored while the Voice & audio card waits for a
push-to-talk key. The choice is kept on this device (`text_scale_percent` in
the local settings, read back clamped to 80-200). Ctrl+Shift+= and the numpad
plus are not bound.

## Icons and tooltips

Icons are Unicode glyphs today. Every icon-only button is an
`ui::kit::icon_button` and always has a tooltip with words (`Icon::label`):
Reply, Edit, React, Copy, Thread, Delete, Attach, Emoji, Send, Mute, Deafen,
Leave and so on. A glyph never stands alone: where the meaning matters the
words are drawn too (the call dock, tags, banners). All glyphs are defined in
one module, `crates/app/src/ui/icons.rs`: `Icon::glyph` gives the text and
`icons::icon` is the single place that turns an `Icon` into a widget. A test
fails when any other source file writes a pictograph or arrow symbol outside
comments and tests. A later move to SVG or an icon font therefore changes
`icons.rs` only (the `glyph` text becomes an image handle or a font code
point, and `icon` draws that); no screen and no tooltip changes. The glyphs
rely on the system's fallback fonts for emoji and symbols (Inter has none of
them); how they look on each OS is on the human checkpoint.

## Narrow windows

The layout follows the window width (`crates/app/src/layout.rs`):

- 1100 px and wider: spaces sidebar, room list (expanded or the rail, as saved
  with Ctrl+B), room pane and the thread or participants panel side by side.
- 800 to 1100 px: while a thread or the participants panel is open the room
  list is drawn as the rail so the room pane keeps its width.
- Under 800 px: the spaces sidebar is not shown, the list is the rail, and an
  open panel takes the room pane's place (only one panel, the one opened last;
  Esc closes the one on screen first and returns to the room). The room pane
  takes no keys while it is hidden, so typing does nothing there.
- The room header stacks its buttons under the name in a narrow pane so
  nothing is cut off; a panel that fills the window repeats the trust tag of a
  direct message.
- The saved room list choice is never changed by the width, and the composer
  keeps its focus through narrowing and widening.
- Known: a resize that changes which list is shown does not keep the room at
  the top, and the spaces sidebar is hidden under 800 px with no sign when a
  space filter is active.

## How calls show state

State is never colour alone:

- Phase: the call dock says it in words ("Connecting", "Voice connected,
  encrypted", "Reconnecting", "Not encrypted") next to a glyph and a dot
  colour. In the rail the same words are in a tooltip, and the phase glyph and
  the lock are always visible.
- Encryption: the lock glyph (encrypted) or the open lock with the words
  "Not encrypted" when media is not encrypted.
- Declined: who declined is written in words in the dock and in the rail (a
  mark plus the words in its tooltip).
- Microphone: an open microphone is the microphone glyph; muted or deafened is
  the crossed microphone and takes precedence; push-to-talk or toggle-to-talk
  that is not transmitting is the key (or gamepad) glyph on an outlined fill;
  transmitting is the microphone on the accent fill. The pill says "Talking" or
  "Not talking: hold F13". Tooltips say Mute / Unmute, Deafen / Undeafen.
- People: speaking is a ring and the word "Speaking"; muted is a mark and the
  word "Muted".
- A ringing call: the ring card with words, the room's call pill, a window
  attention request and a desktop notification. While the card is arming
  (about 0.6 s) its buttons are off and the line reads "Answer in a moment…",
  which also guards against a stray click. The verification buttons (Accept,
  Yes, They match) arm the same way.
- Call notices (failures) are a red banner with words; their close button is
  separate and clicking the words does nothing.

## Known limits of iced 0.14

- Buttons, checkboxes, radio buttons, menus and sliders do not take the
  keyboard focus, so **Tab visits text fields only**; everything else needs
  the pointer. The keys that matter most are chords (ring accept and decline,
  mute, deafen, the switcher) for that reason, and no key presses a button, so
  Enter cannot confirm a destructive action.
- A slider answers the arrow keys only while the pointer is over it; **−** and
  **+** sit beside every slider.
- No accessibility tree (see Screen readers above).
- No file-drop events on native Wayland (see Drag and drop on Wayland).
- A focused text field takes Ctrl+Shift+A and Ctrl+Shift+X (see the keyboard
  map).
- The headless gallery renders with tiny-skia and shows no menus or tooltips;
  the real window uses wgpu. Look in a real window.

## Room dialogs, members, upload, viewer and switcher

- Every dialog has the same shape: a title with a close button (tooltip
  "Close") at the top right, a body that scrolls on its own when the window is
  short (its scrollbar sits beside the content, never over a button), and a
  footer with the main action last. Esc closes or cancels, never confirms, and
  the backdrop takes every click, so nothing behind a dialog can be pressed.
  Clicking outside a dialog does not close it.
- Anything that cannot be undone (leave a room, turn encryption on) keeps its
  question. The button that asks is at the left; the red button with words
  ("Leave", "Enable encryption") that confirms comes up at the right of a
  card, at least 12 px from **Cancel**, never under the pointer that asked.
  Enter, Space and key repeat never press them: no button takes the keyboard
  focus in iced 0.14, and the only fields in these dialogs (name and topic)
  save that field when Enter is pressed in it, nothing else. Tests send the
  keys to each dialog and check that no message comes out.
- The member list (the room header's Members button) has a search field at
  the top (name or user ID, ignoring case and accents), then sections Admins, Moderators, Members and Invited with
  their counts. A row has the picture, the name, the user ID and tags in
  words: the role when the section does not already say it ("Creator",
  "Admin (75)", "Moderator, invited"), the trust of the person when the core
  has reported it ("Verified", "Not verified", "Identity changed", in colour
  and words) and "In the call" while they are in the call. A name that reads
  like one of those words is drawn in quotes, so it cannot pass for a status.
  A long user ID is cut in the middle, so the server name stays visible. The
  list builds 100 rows at a time ("Show more" adds more).
- The upload confirmation names the room the files go to ("To Lounge", with
  whether the room is encrypted) and shows each file's picture or kind, name,
  size and type. **Send** is a separate button from **Cancel**, apart from it,
  and is off while a file is being read, when no file can be sent, or when the
  room on screen is not the room the files were picked for (the card says so).
  A long file name wraps and keeps its extension; a long room name is cut in
  the middle.
- The viewer shows the image centred on a dimmed backdrop, with a bar at the
  bottom: name, type, size and size in pixels, **Save...** and **Close**. Its
  mouse behaviour is the image widget's, unchanged: the wheel zooms the
  picture and dragging moves it. Nothing in it plays or loads beyond the
  decode limits that were there.
- The switcher (Ctrl+K) is a card near the top of the window with a large
  field and one-height rows: the room's picture, name, the call pill and the
  unread badge. Up and Down move the highlight, Enter opens the room, Esc
  closes it; the hint under the list says so. A long room name is cut in the
  middle and wraps inside its row.
