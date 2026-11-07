# The sidebar

How to arrange the left side of Twicecat (the spaces rail and the room list of
a space), where that arrangement is kept, and how to start over. For
keyboard use see [accessibility.md](accessibility.md), section "The sidebar".

## What you can arrange

**The spaces rail**

- Put your spaces in the order you like.
- Group spaces into **folders**. A closed folder shows a 2 x 2 preview of its
  spaces and the total of their unread counts; an open folder lists them. A
  folder can have a name (1 to 32 characters) or none.
- Move a space into a folder, out of it, or into a new folder. Ungroup a folder
  to give its spaces back to the rail. A folder left with one space stays until
  you ungroup it; moving a space out of a folder with two dissolves it.

**The room list of a space**, drawn as sections you can collapse and expand:
Starred, Channels (the space's own rooms), one section per sub-space, and
Direct messages. A collapsed header shows the unread or mention count of its
rooms.

- **Star** a room to put it under Starred. Unstar returns it to its default
  section.
- Make **sections of your own**: create, rename and delete them, and move rooms
  into them. Deleting a section sends its rooms back to their default sections.
- Reorder sections, and reorder rooms inside a section of your own.
- A room can go into Starred or a section of your own, and back to its own
  default section. It cannot be moved from one built-in section to another.

The rooms and spaces themselves are not changed: this is only how your copy
of Twicecat shows them. Nobody else sees your folders and sections, and the
server is not told about them.

**Voice channels** list the people in the call under the room (up to eight,
then "and N more"). The speaking ring and the muted icon are shown only for the
call this device is in.

## How to arrange it

Everything can be done through the **options menus**:

- **right-click** a space, a folder, a room or a section header, or
- press the **"⋯"** button at the right of a section header (or on the open
  room's row).

The menu is a card in the middle of the window with the entries Move up, Move
down, Move to folder (which also offers a new folder), Remove from folder,
Rename, Ungroup, Move to section (which also offers New section), Back to its
default section, Star, Unstar, Collapse, Expand and Delete section.

You can also **drag**: a space onto another one (above or below it, or onto
its middle to make a folder), onto a folder, a room onto a section header or
between the rows of one of your own sections, and a section header onto
another header. Every drop does the same thing as the menu entry of that move;
nothing can be done only by dragging. A drag does not scroll the list, so use
the menu for a target that is out of view. Esc, or leaving the window,
cancels a drag.

No keyboard shortcut was added for any of this. The menus open with a pointer
(a right-click or a click), so someone who uses only the keyboard sees the
arrangement but cannot change it.

## Where it is kept: on this device only

The arrangement is kept **on this device only**. It is not sent to your
homeserver and it is not shared with your other devices or other Matrix
clients: a second computer starts with the default sidebar and has its own
arrangement.

There is **one file per account**, in the same folder as `settings.json`,
named `sidebar-<digest>.json`, where `<digest>` is 16 characters (0 to 9 and
a to f) worked out from the account's user ID. The file holds the account's
user ID, the order, folders and sections, room and space IDs and the names you
gave. It has no message, no room name and no token.

| System | Folder |
|---|---|
| Linux | `$XDG_CONFIG_HOME/twicecat`, or `~/.config/twicecat` when that variable is not set |
| macOS | `~/Library/Application Support/io.github.ga_lep.Twicecat` |
| Windows | `%APPDATA%\ga_lep\Twicecat\config` |

With `--data-dir <dir>` the files are in `<dir>`. With `--ephemeral` the
folder is new and empty each time, so nothing is kept from one run to the next.

- **The file is kept when you log out** and is read again the next time the
  same account logs in on this device, so your arrangement is still there. A
  log out forgets it in memory only; the file stays on disk.
- **Another account on the same device never reads or changes your file.**
  Each account uses its own file, and the account's ID is also written inside
  it and checked, so a file cannot be mistaken for another account's.
- If a file cannot be read at all (it is damaged), it is set aside as
  `sidebar-<digest>.json.bad` and the default sidebar is shown.

## Starting over

- **Reset sidebar layout** in **Settings, Appearance**: press **Reset sidebar
  layout...**, then confirm with **Reset sidebar layout** in the question (or
  **Cancel**). It removes the file of the account you are logged in as, and
  the rail and the room lists go back to the default. It does not touch the
  file of any other account.
- Or quit Twicecat and **delete the file** by hand.

## Limits

The arrangement has caps so that the file stays small. A change that would
pass one is not made.

- Up to 200 entries on the rail (spaces and folders together), at most 50 of
  them folders.
- Up to 20 sections of your own in one space, and up to 500 rooms in its
  Starred and own sections together.
- Folder and section names are 1 to 32 characters, with no control or hidden
  characters.
- Up to 4,000 room and space references in the whole file, and a file larger
  than 256 KiB is not read.

## "Made by a newer Twicecat" and "belongs to another account"

Two kinds of file are **left exactly as they are**. While one is in the way,
the sidebar shows the default arrangement, it cannot be changed, and
**Reset sidebar layout...** is off, with a line that says why on the
Appearance page.

- **Made by a newer Twicecat.** The file says it was written by a later version
  of the program than the one you run now (for example after you went back to
  an older version). This version does not know its format, so it does not
  read it and it never writes over it. Run the newer Twicecat again to use it,
  or delete the file by hand to start over.
- **Belongs to another account.** The file at your account's name says it was
  made for a different account (a file that was copied or renamed by hand, or a
  rare clash of names). It is not yours to read or change, so it is left alone.
  Delete it by hand to start over.
