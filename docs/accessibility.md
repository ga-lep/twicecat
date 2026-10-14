# Accessibility

What Twicecat does for keyboard use, contrast and assistive technology in
Phase 5, and what it does not do yet. Checked against iced 0.14.0 (pinned).

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
| Ctrl+, | Open Settings |
| Ctrl+= / Ctrl+- / Ctrl+0 | Make the text larger / smaller / back to 100 % |
| Ctrl+Shift+M | In a call: mute or unmute the microphone |
| Ctrl+Shift+D | In a call: deafen or undeafen |
| Tab / Shift+Tab | Move to the next / previous field |
| Esc | Close the topmost dialog, panel, Settings or the switcher |
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
- Esc closes one layer per press, topmost first: the incoming-call card
  (hidden, and its ringtone silenced; the room keeps ringing), the Voice &
  audio card, the switcher,
  the upload
  confirmation, the viewer, Settings, a room dialog or panel, then the
  composer's suggestions or reply/edit, then the thread panel, then the
  participants panel of a call. A text field
  that took the Esc to drop its focus does not keep an overlay open.
- Closing the switcher with Esc gives the focus back to the composer.
- Ctrl+K and the room-stepping keys do nothing while another overlay covers
  the main view.
- Ctrl+B works on the main screen only, and also over a dialog. The choice is
  the **Room list** setting, so it is saved. The rail keeps every room-list
  key: Alt+Down / Alt+Up and Ctrl+Shift+Down / Ctrl+Shift+Up step through the
  same list, and the open room stays marked by a fill and a 3 px bar. Its
  avatars are buttons in the same Tab order as the rows; each has a tooltip
  with the room's name and what the dots mean (unread, mentions, in a call,
  ringing, muted), and a dot is never the only sign, since the tooltip words
  say the same. In the rail the search field is a **Search** button that
  expands the list, and the call bar is a column of icon buttons (the call's
  room, mute, deafen, leave), each with its words in a tooltip.
- Ctrl+Shift+M and Ctrl+Shift+D act only during a call, also over a dialog
  (the call goes on behind it); without a call they do nothing.
- While the Voice & audio card waits for a push-to-talk key (**Set key…**),
  every key goes to it and no shortcut runs: the next key with its
  modifiers becomes the binding, a modifier alone keeps waiting, and Esc,
  **Cancel**, closing the card or leaving the window stop waiting.

## Calls

Every call control is a button, a menu or a slider in the widget tree's
order, so Tab reaches it:

- the room header's call button (Call, Join call, Join voice, Leave call)
  and, in the call's room, **People** for the participants panel;
- the call bar at the bottom of the room list: the call's room name (opens
  it with the panel), **Mute**, **Deafen**, **Leave**, and the cross of a
  call notice;
- per person in the participants panel: **−** and **+** (10 % each), the
  volume slider, and the voice cleaning menu;
- the Voice & audio card (from Settings or the call bar's **Audio…**): menus,
  checkboxes, and for each slider **−** and **+** beside it; Esc closes it.
  Its Talking section sets voice activity, push-to-talk or toggle-to-talk,
  the key (**Set key…**, **Clear**), the gamepad button (a menu, so no pad
  is needed to pick one) and the release delay;
- the incoming-call card, a modal layer above everything else: **Decline**
  then **Accept**, and Esc to hide it. While it shows, the room shortcuts
  wait, as under any dialog.

Push-to-talk works from the keyboard alone: the bound key in Twicecat's
window always talks, also with the composer focused (a key that types text
also types it, so a function key or a chord is the better choice), and the
window losing focus lets go. The call bar says "Talking", or which key to
hold or press, in words.

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

## Colour sets, High contrast and zoom

- There are four colour sets ("families"): Twicecat, Frost, Ember and High
  contrast, each with a light and a dark palette (`theme::Family`). Settings,
  Appearance picks the family (four swatches, the one in use is outlined and
  has a check) and the mode (System, Light or Dark). **Use the default** goes
  back to the app's default set (`theme::DEFAULT_FAMILY`).
- All eight palettes are tested for a contrast of at least 4.5:1 (WCAG AA for
  body text) for text on background, muted text, badge and highlight text,
  text on selection, message highlight, hover, surface, raised surface and
  sidebar, the error text on its soft background, and the primary and focus
  colours on the background.
- **High contrast** (black on white, white on black, saturated accents) is
  tested for 7:1 (WCAG AAA) on all of those pairs. Choose it in Settings,
  Appearance, Colours, with Light or Dark mode.
- The ring around a person who is talking (`speaking`) reaches 3:1 against the
  background in all eight palettes (WCAG 1.4.11, non-text contrast).
- Sender names take a colour derived from the user ID and are lightened or
  darkened until they reach 4.5:1 on the current background, in all eight.
- **Zoom** scales the whole window from 80 % to 200 % in steps of 10 %:
  Ctrl+= makes it larger, Ctrl+- smaller and Ctrl+0 resets to 100 % (Cmd on
  macOS). Settings, Appearance has the same as **Smaller**, **Larger** and
  **Reset** buttons. The keys are ignored while the Voice & audio card waits
  for a push-to-talk key. The choice is kept on this device.
- Not tested by code (on the human checklist): that the keyboard focus is
  visible on every control in both themes, since it depends on iced's widget
  styles.
