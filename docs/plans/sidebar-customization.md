# Sidebar Customization Plan
Status: APPROVED

## Goal
Make the desktop app's left side work like the "Matrix Spaces Sidebar" design (boards 4 "Rail item states", 5 "Slack-style text channel + thread" and 6 "Discord-style voice channel"), and let the user arrange it:

1. **The spaces rail.** Spaces can be reordered and grouped into folders. A collapsed folder shows a 2 × 2 preview of its spaces, and an open one lists them. Each item has the RailStates looks:
   - default: a circle, no pill;
   - hover: a squircle, a 20 px pill and a tooltip after 500 ms;
   - unread: an 8 px pill, rolled up from every room in the space;
   - mentions: a count badge;
   - selected: a squircle, a 32 px pill and a ring.
2. **The room list inside a space, as collapsible sections:**
   - Starred;
   - Channels (the space's own rooms);
   - one section per sub-space (Tech, Hobbies, Voice, ...);
   - Direct messages.

   A collapsed header shows the rolled-up unread or mention count of its rooms.
3. **User-defined sections:** create, rename, delete and reorder sections, and move rooms between them.
4. **Voice channels** in the list, with the people in the call under them, as in board 6. People who are speaking get a ring and muted people get an icon, where the app can know it (see decision 6).

Every arrangement is stored **locally**, in a file of its own per account (decision 1). The app reaches the storage only through a small storage trait (`SidebarStore`), so Matrix account-data sync can be added later without changing the UI. That sync is not planned here: its open issues are listed under Questions (Q14). The local file has a schema version. It is read under hard limits, field by field, and validated. It is bound to its account: it is kept at logout, and another account never reads or overwrites it. Room names and IDs never appear in `Debug` output or in logs.

Dragging is never the only way to do something. Every move also exists as an action in the right-click / "⋯" options menus (Move up, Move down, Move to folder, Move to section, Star). **No keyboard chord is added** (Q12), and every existing shortcut keeps its binding and meaning. Because iced 0.14 gives buttons no keyboard focus (`docs/accessibility.md`, Settings section), the menus are opened with a pointer. Arranging the sidebar is therefore a pointer feature. Keyboard-only users keep the default arrangement and every existing way to move around (decision 5, "Keyboard-only users"). This limit goes on the human checkpoint and in PROGRESS.md's open issues.

**Where the work goes (decision 0, Q1):** on the current branch, `feat/ui-ux-rework`. No new branch is made, and opening a PR is not part of this plan. Commits carry `Step: sbc.N` trailers. The ledger is `docs/plans/sidebar-customization.progress.md`.

### What exists today (surveyed at `f035942`, tip of `feat/ui-ux-rework`)
- **Branches.** The checkout is on `feat/ui-ux-rework`, not `feat/call-sounds`. `feat/call-sounds`, which holds the call sounds and the custom theme, was already merged into it (`f035942 docs(plan): merge feat/call-sounds into feat/ui-ux-rework`, PROGRESS.md "Merge of feat/call-sounds into feat/ui-ux-rework"). `main..HEAD` is 117 commits, and no PR is open for either branch (`gh pr list`). The tree is clean.
- **Spaces in the core.**
  - `crates/core/src/spaces.rs` forwards `matrix_sdk_ui::spaces::SpaceService::subscribe_to_space_filters` as `CoreEvent::Spaces(Vec<SpaceView>)`. `SpaceView` is `{ id, name, avatar, level, children: u64 }`.
  - The core keeps each space's `descendants` (the `SpaceChildren` watch) only for the room list filter. The app never receives them.
  - In `matrix-sdk-ui 0.19.1` (`src/spaces/mod.rs:689-750`, `:773-785`), filters stop at 2 levels:
    - level 0 holds a top-level space's **direct** children, top-level spaces sorted by `order`, then by room ID (`compare_top_level_space_rooms`, `:756`);
    - level 1 holds each child space, sorted by the `m.space.child` `order`, then by timestamp, then by room ID (`SpaceRoom::compare_rooms`, `room.rs:170`), with **all** of its descendants flattened (`flattened_bottom_up_subtree`).
  - Level-2 spaces (for example "Rust" inside "Tech" in board 5) are not filters. The order of rooms *inside* a space (`SpaceRoomChildState`) is `pub(crate)`, so it cannot be read.
- **Spaces in the app.**
  - `crates/app/src/spaces.rs` has `SpacesModel` (capped at `MAX_SPACES = 200`; `Debug` shows counts only), `entries()` (top-level spaces, plus the level-1 children of the selected space) and `SpacesMessage::Select`.
  - `screens/spaces_sidebar.rs` draws a 64 px column (`SIDEBAR_WIDTH`, also used by `main_view.rs:174`): Home, then a round 40 px `button(...).style(button::text)` per space, with a 3 px `primary` ring when selected and a tooltip on the right. There is no unread state, no hover look and no order of its own.
- **The room list.**
  - `RoomListQuery { tab, search, space }`. The core filters to the selected space's descendants and pages by 500 (`DEFAULT_ROOM_LIST_PAGE_SIZE`, `LoadMoreRooms`).
  - `RoomSummary` has `unread_messages`, `unread_notifications`, `highlights`, `marked_unread`, `is_dm`, `is_space`, `is_call_room` and `notification_mode`. It has **no** favourite tag and **no** parent space.
  - `room_list.rs` has `badge_for` (Highlight, Count or Dot) and fixed-height windowing (`visible_range` and `spacer_heights`, which take one `row_h`).
  - `room_list_pane.rs` keeps five slots (actions, tabs, search, list, dock) and has an expanded mode and a rail mode.
  - Alt+Up and Alt+Down walk `app.rooms` in order.
- **Unread totals.** `CoreEvent::UnreadTotals { notifications, highlights }` covers all joined rooms that are not muted (`notifications.rs:398-450`, at most once a second). There is no total per space.
- **Calls.**
  - `CoreEvent::ActiveCall { room_id, participants: u32 }` gives a device count per room. The watcher computes it from `active_memberships` (`call/watcher.rs:528-562`) and sends it only when the count changes.
  - `CoreEvent::CallParticipants` gives `Participant { user_id, display_name, muted, speaking, .. }` for **this device's** call only.
  - `calls.rs` keeps `counts` and `call.participants`.
- **Local settings.**
  - `DataPaths` (`crates/core/src/paths.rs`) reads `<root>/settings.json` through `read_capped_regular_file` (a regular file of at most 64 KiB; anything else gives the defaults). It parses field by field (`settings_from_map`), renames a corrupt file `settings.json.bad`, and writes atomically through a hand-built `serde_json::json!` (the serializer guard in `session.rs:296-347` forbids `Serialize` derives and `serde_json::to_*` outside its allow list).
  - `UiSettings` is `Copy`.
  - `ThemeName` (`api/rooms.rs:363-447`) is the model for a validated `Copy` name with a safe `Debug`.
  - The app saves through the `SettingsStore` trait (`settings.rs:337-391`). `DiskSettings` coalesces writes, latest wins, under `blocking::run`, and `NoSettingsStore` is the test default.
- **One account at a time.** `last_account` holds one user ID, and logout removes the session store (`engine/mod.rs:2545`, `:2886`).
- **Accessibility.** iced 0.14 gives only text fields keyboard focus (`docs/accessibility.md`, Settings section). The `SHORTCUTS` table in `keys.rs` drives both the key handling and the Settings list. Every new menu goes through `kit::menu` inside `ui::guard::no_wheel` (`menus_ignore_the_wheel.rs`).

### iced 0.14 capabilities checked for this plan (rule 2; registry source of the locked versions: `iced 0.14.0`, `iced_widget 0.14.2`, `iced_core 0.14.0`)
- **iced has no drag-and-drop widget.** `iced_widget-0.14.2/src` has no draggable or droppable list. The only drag code is `pane_grid/draggable.rs`, which is internal to the pane grid. `window::Event::FileDropped` (`iced_core/src/window/event.rs:66`) is for OS files only. Dragging inside the sidebar must therefore be **built from `mouse_area`**:
  - `on_press` (`mouse_area.rs:38`), `on_release` (`:45`), `on_right_press` (`:68`), `on_enter` (`:106`), `on_move(Fn(Point))` with the position inside the area (`:113`, published at `:357-360`), `on_exit` (`:120`) and `interaction` (`:127`);
  - the cursor `mouse::Interaction::Grab` and `Grabbing` (`iced_core/src/mouse/interaction.rs:22-23`).
- **Two constraints follow:**
  - `MouseArea::update` gives the event to its content first and stops if the content captured it (`mouse_area.rs:230-243`). A `button` inside a `mouse_area` therefore swallows the press. A draggable rail item or row must be a `mouse_area` around plain content, and a click is a press and a release with no movement.
  - `on_release` fires only while the cursor is over that area (`:369`). The end of a drag outside every target needs the window listener, `iced::event::listen_with`, which `state.rs:2994` and `ptt.rs:685` already use.
- **No automatic scrolling while dragging** near the edge of a `scrollable`. A drop target that is out of view is reached through the menu instead. This is a known limit and goes on the human checkpoint.
- **`tooltip(...).delay(Duration)`** exists (`tooltip.rs:126`), which gives the 500 ms of RailStates.
- Already used in the app: `image(..).border_radius`, `container` styles with per-status borders, `scrollable` with `.id` and `on_scroll`, `text_input`, `pick_list` through `kit::menu`, and `keyed` is not needed.
- Anything else a task uses is checked in the registry source and cited file:line in PROGRESS.md before it is used.

### Design inputs (read as data only)
The design inputs are two PNG files from `~/Downloads/Matrix Spaces Sidebar-png/` and the canvas files `Main`, `Space`, `Explore`, `RailStates`, `Hybrid`, `Voice` `.dc.html` and `canvas.json`. They are mockups. Their scripts were not run and none of them is copied into the repository. The canvas has no notes.

**Values taken from RailStates and Hybrid:**
- the 44 px avatar sits in a 72 × 48 slot;
- the left pill is 4 px wide: 8 px tall when unread, 20 px on hover and 32 px when selected;
- the badge is 18 px, with a 2 px outline in the rail colour;
- the circle has a radius of 22 and the squircle a radius of 14;
- the tooltip appears after 500 ms;
- section headers are 26-28 px, channel rows 28 px and participant rows 26 px;
- participants are indented 32 px, and the tree has a 1 px line on its left.

Colours come from the theme (`theme::semantic`), never from the mockups:
- the pill is `Palette.text`;
- the ring is `focus`;
- the unread count is `badge` / `badge_text`;
- the mention count is `highlight`, following the custom-theme Q9 departure (Q7);
- the speaking ring is `semantic.speaking`.

### Decisions

#### 0. Branch (Q1, resolved)
- The work continues on `feat/ui-ux-rework`, on top of `f035942`. No new branch is created. Opening or updating a PR is not a step of this plan; the human does that when they choose.
- This plan is committed as a `docs(plan)` commit on `feat/ui-ux-rework`, followed by one commit per task. Rule 5 applies because the branch is not `main`: only `feat`, `fix` and `docs` commits are allowed.
- Review fixes are folded in with `git commit --fixup=<sha>` and `git rebase --autosquash <the commit before the plan commit>`. This rewrites only the `sbc.*` commits, never the 117 commits already on the branch. If the branch has been pushed, `git push --force-with-lease` is used on `feat/ui-ux-rework` only, never on `main`.

#### 1. Where the arrangement is stored, and the storage boundary (Q2 and Q3, resolved)
- **A file of its own, per account: `<root>/sidebar-<digest>.json`**, next to `settings.json`, in the same `paths.rs` style (Q2: its own sidebar file, not a key in `settings.json`). `<digest>` is defined under "Account binding" below. In this plan "the sidebar file" means that per-account file. A key inside `settings.json` was rejected for three reasons:
  - **Size.** Room IDs can be 255 bytes long. Even a capped layout can pass `settings.json`'s 64 KiB cap, and an oversized `settings.json` is read as missing, so the user would lose **every** setting.
  - **Corruption.** A corrupt layout must not rename the user's other settings `settings.json.bad`.
  - **Type.** `UiSettings` is `Copy`, and a layout holds `Vec`s.
- **The format, version 1:** JSON, hand-built with `serde_json::json!`, with no `Serialize` derive (to satisfy the serializer guard).

  ```json
  {
    "version": 1,
    "account": "@user:server",
    "rail": [
      {"space": "!a"},
      {"folder": {"id": 1, "name": "Work", "open": false, "spaces": ["!b", "!c"]}}
    ],
    "spaces": [
      {
        "space": "!a",
        "order": ["starred", "channels", "sub:!t", "custom:2", "dms"],
        "collapsed": ["sub:!t"],
        "starred": ["!r1"],
        "custom": [{"id": 2, "name": "Ops", "rooms": ["!r2", "!r3"]}]
      }
    ]
  }
  ```

- **Reading** (`DataPaths::read_sidebar(account)`, which reads only that account's file):
  - `read_capped_regular_file`, made to take a cap, with `MAX_SIDEBAR_BYTES = 256 KiB`. A symlink target that is not a regular file, a device, a FIFO or a larger file reads as "no layout". The settings cap stays 64 KiB.
  - `serde_json::Value` and its recursion limit, then validation field by field, as `settings_from_map` does.
  - `version` must be exactly `1`:
    - no version, or one that is not an integer: corrupt;
    - a **higher** version: `SidebarRead::Newer`. The file is never overwritten or renamed. The app shows the default arrangement and turns customizing off with one line, "Your sidebar layout was saved by a newer Twicecat", so a downgrade cannot destroy it (Q4, resolved).
  - `account` is checked next (see "Account binding"). A file whose `account` is missing, invalid or another user's reads as `SidebarRead::OtherAccount` and is never renamed.
  - Not JSON, or not an object: the file is renamed `sidebar-<digest>.json.bad` and the app starts from the default layout. `settings.json` and other accounts' files are never touched.
  - An entry that fails validation is dropped and the rest is kept. Duplicates keep the first:
    - a space appears once in the rail, folders included;
    - a room appears once per space across Starred and custom sections;
    - section keys are unique.
- **Validation rules:**
  - **Room IDs:** start with `!`, 2 to 255 bytes, ASCII graphic only (`0x21..=0x7E`). There is no `:server` requirement, because room version 12 IDs have none.
  - **`account`:** an `@` user ID with the same byte rules.
  - **Folder and section IDs:** `u16`, unique within their scope.
  - **Names:** `SidebarLabel`, which is `Copy` (a `[u8; 96]` and a length):
    - 1 to 32 characters (at most 96 UTF-8 bytes), trimmed;
    - no control, format or bidi characters (the same characters `text_safety` strips);
    - shown through `text_safety`;
    - `Debug` is `<redacted, N bytes>`.
  - **Section keys:** `starred`, `channels`, `dms`, `sub:<room id>` and `custom:<id>`.
- **Caps** (constants in `api::sidebar::limits`, each tested):
  - rail entries ≤ 200 (`MAX_SPACES`);
  - folders ≤ 50, and spaces in all folders ≤ 200;
  - per-space records ≤ 200;
  - custom sections per space ≤ 20;
  - Starred plus custom rooms per space ≤ 500;
  - **room references across the whole file ≤ 4,000, and the bytes of all IDs ≤ 192 KiB**, so a file the app writes always fits under `MAX_SIDEBAR_BYTES`.
  - An edit that would pass a cap is refused, and the page says "The sidebar layout is full". It never truncates silently.
- **Writing** (`DataPaths::write_sidebar(&SidebarLayout)`, to the file of `layout.account`):
  - atomic, with the settings code's protections: remove a stale temp file or symlink first, mode 0o600 on Unix, `sync_all`, then rename;
  - written only after a user action, never at start-up;
  - **guarded:** before writing, the existing file at that path is read as above. If it is `Newer` or `OtherAccount`, the write is refused with `SidebarWriteError::Protected` and the file stays byte for byte the same. This check in `paths.rs` holds even if the app's model state were wrong.
- **Account binding (Q3, resolved): one file per account, plus a field check.**
  - **File name.** `<digest>` is the 64-bit FNV-1a hash of the user ID's UTF-8 bytes, as 16 lowercase hex digits, for example `sidebar-1f2e3d4c5b6a7980.json`. FNV-1a is a fixed public algorithm, written out in `paths.rs` (about ten lines, tested against the published vectors, for example the empty string gives `cbf29ce484222325`). It is not `std`'s `DefaultHasher`, whose output may change between Rust releases, and no hashing crate is added (rule 3). The name keeps user IDs (up to 255 bytes, with `@` and `:`) out of file names and is always a valid name on Windows. It is not a secret: a user ID can be guessed and hashed.
  - **Field check.** The file's `account` must equal the logged-in user ID exactly. A mismatch (only possible through a hash collision or a hand-edited or planted file) reads as `OtherAccount`: the app shows the default arrangement, turns customizing off with one line, "This device's sidebar layout file for your account belongs to another account", and never writes, renames or removes that file.
  - **What happens, precisely:**
    - logging in as `@a` reads only `sidebar-<digest(@a)>.json`;
    - **logout does not touch any sidebar file**. The app drops the layout from memory only;
    - logging in later as `@b` reads `sidebar-<digest(@b)>.json`, which is missing, so `@b` starts from the default arrangement. `@b`'s changes go to `@b`'s own file. `@a`'s file is never read, written, renamed or removed while `@b` is logged in;
    - logging back in as `@a` restores `@a`'s layout.
  - Account switching follows `last_account`: one account is logged in at a time, so only one sidebar file is open at a time.
- **Stale entries:**
  - spaces and rooms that the core does not currently report are kept in the file (sync may not have delivered them yet) but are not drawn;
  - they count toward the caps;
  - **Reset sidebar layout** (Settings → Appearance) removes the logged-in account's file after a confirmation, and only that file. It is turned off, with the matching note, in the `Newer` and `OtherAccount` states, so it cannot remove a file this app may not touch.
- **Privacy:** a file holds room IDs, the user's own folder and section names, and the account ID. It holds no room names. Nothing reads or writes these files except `paths.rs`, and nothing logs them. Because files are kept at logout (Q3), the files of logged-out accounts stay on disk. `docs/sidebar.md` says so and says how to remove them: Reset while logged in as that account, or delete the file.
- **The boundary** (app side, `crates/app/src/sidebar_store.rs`):

  ```rust
  pub trait SidebarStore: Send {
      /// The stored layout for `account`; resolves to `SidebarLoad::{Layout, Default, Newer, OtherAccount, Unavailable}`.
      fn load(&self, account: &UserId) -> Task<SidebarLoad>;
      /// Writes `layout` to `account`'s file; coalesced, latest wins; yields whether it worked
      /// (false when the file is protected: `Newer` or `OtherAccount`).
      fn save(&self, account: &UserId, layout: SidebarLayout) -> Task<bool>;
      /// Removes `account`'s file (Reset only; never called at logout).
      fn reset(&self, account: &UserId) -> Task<bool>;
  }
  ```

  - `DiskSidebar` implements it over `DataPaths`, with reads and writes in `blocking::run`, as `DiskSettings` does.
  - `NoSidebarStore` is the test default and does nothing.
  - `MemorySidebar` is a test fake that records calls.
  - The UI calls only `SidebarModel` methods, and the model calls only the trait. A later `AccountDataSidebar`, through a core command, would be another implementation. It is **not** planned here.

#### 2. Data the core adds (minimal, additive; each is its own `feat(core)` step)
- **D1, a space's rooms** (Task 3):
  - `SpaceView.rooms: Vec<RoomId>` carries the SDK filter's `descendants` unchanged, capped at 2,000 (`MAX_SPACE_ROOMS`). For level 0 these are the direct children (rooms and sub-spaces); for level 1, all descendants.
  - `Debug` shows the count only.
  - This is what the app needs to sort a space's rooms into Channels, sub-space sections and Direct messages.
  - It breaks the `SpaceView` struct literal, which only the app's tests and the CLI build.
- **D2, unread totals per space** (Task 4): `CoreEvent::SpaceUnread { spaces: Vec<SpaceUnreadView>, outside: UnreadCountsView }`, where `SpaceUnreadView { space: RoomId, counts: UnreadCountsView }` and `UnreadCountsView { notifications: u64, highlights: u64, unread: bool }`.
  - It is computed in the core over **all** joined rooms. The app's room list only holds the selected space's rooms, so it cannot compute this itself.
  - It uses the totals rule: muted rooms are left out, and sums saturate.
  - A top-level space's counts cover its direct rooms and the descendants of its level-1 children.
  - `outside` counts rooms in no space, DMs included, which gives the Home badge.
  - It runs on the `run_totals` machinery: sent at most once a second, and only on a change.
  - DMs inside a space count as rooms of that space. The design's "DMs in the space count too" is left at that (Q8).
- **D3, who is in a voice room** (Task 11): `CoreEvent::ActiveCall` gains `members: Vec<CallMemberView>`, where `CallMemberView { user_id, display_name: Option<String>, avatar: Option<AvatarId> }`.
  - Members are distinct users, in join order, capped at 25. `participants` stays the device count.
  - It is sent when the member set changes, not only when the count changes.
  - Names come from the room's member store, cleaned to one line as other names are. `Debug` hides them.
  - `CallMemberView` is a new public type in `api/call.rs`.
- **No order field is added.**
  - The rail keeps the core's top-level order (MSC3230) as its default.
  - Sub-space sections follow the core's level-1 order, which already uses `m.space.child.order`.
  - Rooms inside a section follow the room list's order (recent activity first), because the SDK keeps child order `pub(crate)` (Q6).
- **Level-2 sub-spaces** (such as "Rust" inside "Tech") are not filters in the SDK. Their rooms appear flat inside the level-1 section, and no nested "Rust" header is drawn (Q5).

#### 3. Rail look and behaviour (RailStates)
- `SIDEBAR_WIDTH` becomes 72. Each item is a 72 × 48 slot with a 44 px icon. `main_view.rs:174` follows the constant.
- **The pill**, at the left edge, 4 px wide, coloured `Palette.text`:
  - none by default;
  - 8 px tall when unread (`counts.unread` or `notifications > 0`);
  - 20 px on hover;
  - 32 px when selected.
- **The shape:** a circle (radius 22) when idle, a squircle (radius 14) on hover or when selected, and a 2 px `focus` ring when selected.
- **The badge**, at the bottom right, 18 px: highlights in `highlight` and `badge_text`, else the notification count in `badge`, shown as "99+" above 99. A dot is not drawn on the rail; the pill shows plain unread.
- **Hover** is tracked through `mouse_area` `on_enter` and `on_exit` in `App.rail_hover: Option<RailKey>`, because an `image` cannot change its radius by button status.
- **The tooltip** is the space's name through `text_safety`, after 500 ms.
- **Home** keeps its first slot. Its badge and pill come from `SpaceUnread.outside`.
- **Sub-spaces leave the rail** (Q5, resolved). They become sections of the room list (decision 4), as in boards 5 and 6. Selecting a sub-space from the rail goes away; clicking a sub-space section header collapses it.
- **Folders:**
  - collapsed, a folder is a 44 px squircle holding a 2 × 2 grid of its first four space icons at 18 px;
  - open, its spaces are listed under it, indented 4 px, with a tinted band behind them;
  - clicking toggles it, and the state is saved;
  - the folder's badge and pill are its spaces' totals;
  - a folder's optional name is its tooltip ("Folder" when unnamed);
  - a folder left with fewer than 2 spaces dissolves into the rail (Q9).

#### 4. Sections inside a space
- **When sections show:** a space is selected, the tab is All, the search is blank and the room list is expanded. Otherwise, with a search, the People or Rooms tab, Home or the collapsed rail, the list stays flat as today.
- **Where a room goes:** each room of the space appears exactly once, in the first of these that applies:
  1. **Starred**, if starred in this space. Starred is local and per space only; rooms with the Matrix `m.favourite` tag are **not** added, and the core gains no favourite field (Q10, resolved);
  2. **a custom section**, if the user put it there;
  3. **the sub-space section** of the level-1 child whose `rooms` contain it. A sub-space room that is itself a voice room stays there, as in board 6;
  4. **Direct messages**, if `is_dm`;
  5. **Channels**, otherwise. The sub-space rooms themselves (`is_space`) are not drawn as rows.
- **Default order:** Starred, Channels, the sub-spaces in core order, the custom sections in creation order, then Direct messages. A stored `order` overrides it. Keys missing from it go to their default place, and stored keys that are unknown are kept but not drawn.
- **Hidden sections:** an empty Starred or Direct messages section is hidden, and so is an empty sub-space section. An empty custom section shows its header with the line "Drop or move rooms here".
- **Headers:**
  - a chevron, the name (sub-space names through `text_safety`; "Starred" with "personal" in muted text at the right, as in the boards; the user's own label for custom sections), and an options button "⋯" on the right;
  - a collapsed header shows the rolled-up badge of its rooms, using `badge_for`'s priority over the summed counts (highlights, then notifications, then a dot);
  - clicking a header toggles it, and the state is saved per space.
- **Windowing:**
  - the list becomes a flat list of `Line`s (`Header`, `Room`, `Participant`, `More(n)`, `EmptyHint`), each with its own height;
  - windowing uses prefix sums of the heights and a binary search (`line_offsets`, `visible_lines`), so only visible lines are built, as today;
  - rows keep the current `room_row` and `rail_row` look and density heights (Q11, no row redesign).
- **Loading:** while sections show, the app asks for more pages (`LoadMoreRooms`) until the space's rooms are all loaded, because sections need every room to be placed. The core's 500-room page and total bound this.
- **Keyboard (existing keys only):** the existing Alt+Up/Down and Ctrl+Shift+Up/Down keep their binding and meaning, "the next or previous room in the list as shown". In sectioned mode the list as shown is the drawn room lines in order, so rooms in collapsed sections are skipped. No `SHORTCUTS` row is added or changed.

#### 5. Moving things without dragging (accessibility)
- **The options card.** Opened by right-click (`mouse_area.on_right_press`) on a rail item, a section header or a room row, or by the "⋯" on headers and on the selected room's row. It uses `kit::card`, with plain buttons; any pick list goes through `kit::menu` inside `no_wheel`. Esc closes it, and it takes its place in the Esc order after the switcher.
  - **Space:** Move up, Move down, Move to folder (the folders, then "New folder"), Remove from folder.
  - **Folder:** Rename, Move up, Move down, Ungroup.
  - **Section:** Rename, Move up, Move down, Collapse/Expand, and Delete (custom sections only; their rooms return to their default sections).
  - **Room:** Star/Unstar, Move to section (the custom sections, then "New section..."), Move up and Move down (custom sections only, where the order is the user's), and Back to its default section.
- **No keyboard chords (Q12, resolved).** The menus are the only non-drag path for every move, folder move, section move and star. `SHORTCUTS` in `keys.rs` gets no new row, and no existing row changes. Its Settings list and the keyboard table in `docs/accessibility.md` stay as they are. The only keys these tasks touch are ones that already exist:
  - Esc closes the options card or the name dialog, as one more layer in the existing Esc order;
  - Enter in the name dialog's `text_input` submits it, through the field's own `on_submit`, as other dialogs do.
- **Names:** "New folder", "New section..." and Rename open a small `kit::dialog` with one `text_input`, which takes the focus. It accepts at most 32 characters and is validated with `SidebarLabel`. Enter saves and Esc cancels.
- **Keyboard-only users (an honest limit).** iced 0.14 gives buttons and `mouse_area`s no keyboard focus, and Tab visits only text fields. With no chords, nothing opens an options card from the keyboard. So:
  - **what keyboard-only users keep:** the whole app works with the default arrangement and with an arrangement made earlier with a pointer. Ctrl+K, Alt+Up/Down and Ctrl+Shift+Up/Down reach every drawn room. Sections are drawn expanded by default, so no room is hidden from the keyboard unless a pointer user collapsed its section. Once a pointer has opened the name dialog, its text field, Enter and Esc work from the keyboard;
  - **what they cannot do:** open the options menus, so they cannot reorder spaces, make or change folders, star rooms, create, rename, delete, reorder, collapse or expand sections, or move rooms between them. They also cannot expand a section that was collapsed, and rooms in it are skipped by the room-stepping keys. Ctrl+K still finds and opens those rooms;
  - this is stated in `docs/accessibility.md` and `docs/sidebar.md` (Task 13), recorded in PROGRESS.md's open issues from Task 6 onward, and put on the human checkpoint. A keyboard path would need either chords, which the user turned down, or keyboard focus for buttons in a later iced. Neither is planned here.

#### 6. Drag and drop (pointer users)
- **A state machine in `crates/app/src/drag.rs`**, tested without a window: `Idle`, then `Pressed { item, at }`, then `Dragging { item, target }` after 4 px of movement, then `Drop` or `Cancel`.
  - The press comes from the item's `mouse_area.on_press`, the movement from `on_move` and `on_enter`, and the end from a window `listen_with` on `ButtonReleased(Left)`.
  - Esc, the window losing focus, or a release with no target cancels.
  - A press and release with no movement is a click: it selects the item or toggles the folder.
  - The cursor shows `Grab` on hover and `Grabbing` while dragging.
- **Drop targets:**
  - **on the rail:** the top or bottom third of an item means before or after it; the middle third of a space means "make a folder with it", or "put it in this folder" on a folder. A thin `focus` line or ring shows the target;
  - **in the room list:** a room onto a header or between rows of a custom section moves it there (into Starred, a custom section, or back to its default section); a header dropped onto a header reorders sections.
- **What a drop does:** each drop calls the **same** `SidebarModel` operation as the menu action. A test enforces this: every `DragDrop` maps to an operation that the menu also offers.
- **No auto-scroll** (iced limit). Targets out of view are reached by the menu, and this goes on the human checkpoint.

#### 7. Voice channels in the list
- A voice room (`is_call_room`) with members draws one `Participant` line per member under it:
  - a 20 px avatar, through `Avatars` (avatars requested once, as for spaces);
  - the name through `text_safety`, falling back to the localpart of the user ID;
  - "and N more" after 8 lines;
  - and the device count on the room row (`calls.count`), as today.
- **Speaking and muted are known only for the call this device is in.** For that room, the lines join `CallParticipants` by `user_id`: speaking draws a 2 px `semantic.speaking` ring, and muted draws the existing muted icon in `danger`. Other rooms' members show neither. The MatrixRTC membership state does not carry them, and asking the SFU would mean joining (Q13).
- The participant lines of a voice room in a collapsed section are hidden with the section. Clicking a participant line selects the voice room.

### Rules for every task (AGENTS.md)
- **Rule 1:** this plan starts only after the human approves it. Each task starts only after the previous one's acceptance checks pass.
- **Rule 2:** the iced APIs are cited above. The SDK API is `matrix-sdk-ui 0.19.1`: `SpaceFilter.descendants` (`spaces/mod.rs:784`) and `subscribe_to_space_filters` (`:299`). D3 uses the core's existing `active_memberships` (`call/watcher.rs:530`) and member lookups already used by `RoomMembers`. Any other API is checked in the registry source and cited in PROGRESS.md before use.
- **Rule 3:** no version changes and no new crates. `serde_json` is already a dependency of both crates. The file-name digest is a hand-written FNV-1a, not a hashing crate.
- **Rule 4, the gate before every commit:**
  - `cargo fmt --all`;
  - `cargo clippy --workspace --all-targets -- -D warnings`, and again with `--all-features`;
  - `cargo test --workspace` and `cargo check --workspace --lib --bins --locked`;
  - `scripts/check-app-deps.sh`, `scripts/check-core-api.sh`, `bash scripts/check-font-assets.sh`, `bash scripts/check-no-webrtc-download.sh` and `scripts/check-commits.sh main` (the branch is `feat/ui-ux-rework`, whose base is `main`);
  - `cargo deny check`.
- **Rule 5:**
  - one `feat(core)` or `feat(app)` commit per task, with the trailer `Step: sbc.N`;
  - plan, ledger and evidence commits are `docs(plan)`, and Task 13 is `docs(app)`;
  - all of it on `feat/ui-ux-rework` (decision 0); review fixes are folded in with `--fixup` and `--autosquash`, only within the `sbc.*` commits;
  - every commit updates `PROGRESS.md` ("Sidebar customization steps", "Sidebar customization human checkpoint" and its open issues, which carry the keyboard-only limit from Task 6 onward) and the ledger.
- **Rule 6:** the core integration tests (Tasks 3, 4 and 11) run only against the `docker-compose.yml` homeserver with the `.env.test` accounts (`scripts/require-local-homeserver.sh`). Their spaces, rooms and calls are made up. Nothing logs IDs, names or message bodies; `no_secret_logging.rs` and `call_log_hygiene.rs` still pass.
- **Rule 7:** the app reaches the new data only through `twicecat_core::api` (`SidebarLayout`, `SpaceView.rooms`, `CoreEvent::SpaceUnread`, `CallMemberView`) and `twicecat_core::paths`. `check-app-deps.sh` and `check-core-api.sh` pass after every task.
- **Rule 8:** stop and ask if an SDK API named here behaves differently than its source reads (for example, if level-0 `descendants` turn out not to include sub-space IDs), if a homeserver test is flaky twice in a row, or if a step would rewrite another plan's code beyond the files listed.
- **Rule 9:** how the rail, folders, sections, drag feedback, tooltips and voice lines look in a real window goes on the human checkpoint. The gallery's PNG images are an aid, not proof.
- **Lessons carried over from earlier plans:**
  - TDD: the ledger says honestly which tests were red first;
  - every mutation is named and reverted;
  - every new menu goes through `kit::menu` inside `no_wheel`;
  - trust and badge colours are checked by distance, not only by `assert_ne!`;
  - the gallery's layout and overlap checks run at 1280 and 720 wide.

### Task 1: The sidebar layout type and its local file (core)
Language: rust
Files:
- `crates/core/src/api/sidebar.rs` (new):
  - `SidebarLayout { account, rail: Vec<RailEntry>, spaces: Vec<SpaceLayout> }`;
  - `RailEntry::{Space(RoomId), Folder(FolderLayout)}`, `FolderLayout { id: u16, name: Option<SidebarLabel>, open: bool, spaces }`;
  - `SpaceLayout { space, order: Vec<SectionKey>, collapsed: Vec<SectionKey>, starred: Vec<RoomId>, custom: Vec<CustomSection> }`, `CustomSection { id: u16, name: SidebarLabel, rooms }`;
  - `SectionKey::{Starred, Channels, Direct, SubSpace(RoomId), Custom(u16)}`;
  - `SidebarLabel` (`Copy`, `TryFrom<&str>`, `as_str`, redacted `Debug`, doctests);
  - `SidebarRead::{Layout(SidebarLayout), Missing, Corrupt, Newer(u64), OtherAccount}`;
  - `limits::{MAX_RAIL_ENTRIES, MAX_FOLDERS, MAX_SPACE_RECORDS, MAX_CUSTOM_SECTIONS, MAX_PLACED_ROOMS, MAX_ROOM_REFS, MAX_ID_BYTES_TOTAL, MAX_SIDEBAR_BYTES}`;
  - `SidebarLayout::fits()` (the budget check) and `normalized()` (dedup and caps);
  - `Debug` shows counts only.
- `crates/core/src/api/mod.rs`: `pub mod sidebar;` and re-exports.
- `crates/core/src/paths.rs`:
  - `read_capped_regular_file(path, cap)`;
  - `fnv1a_64(bytes) -> u64` (private) and `sidebar_file(account) -> PathBuf` (`<root>/sidebar-<16 hex>.json`);
  - `read_sidebar(account) -> SidebarRead`, `write_sidebar(&SidebarLayout) -> Result<(), SidebarWriteError>` (guarded: `Protected` for a `Newer` or `OtherAccount` file at the target path) and `remove_sidebar(account)` (also refuses a protected file);
  - `sidebar_from_value(value, account)` (field by field, with the validation, the caps and the account check of decision 1);
  - tests.
- `PROGRESS.md` and the ledger.

Depends on: none
Acceptance criteria:
- Round trip: a layout built from every variant (folders open and closed, named and unnamed; every `SectionKey`; Starred; custom sections) passes through `write_sidebar` and `read_sidebar` unchanged. The written file has `"version":1` and mode 0o600 on Unix, and no `.tmp` file is left behind.
- Missing file: `Missing`. Not JSON, a JSON array, or a missing or non-integer `version`: `Corrupt`, and the file is renamed `sidebar-<digest>.json.bad`, while `settings.json` in the same folder stays byte for byte the same. `"version":2`: `Newer(2)`, and the file is neither renamed nor changed.
- File name: `sidebar_file` gives `sidebar-` + 16 lowercase hex digits + `.json`; `fnv1a_64` matches the published FNV-1a 64 test vectors (the empty string gives `cbf29ce484222325`); the name is stable across runs (a pinned expected name for `@alice:example.org`); two different user IDs give two different files; a 255-byte user ID still gives a 30-character name.
- **Another account's file is ignored and never overwritten:**
  - with `@a:x`'s layout written, `read_sidebar("@b:x")` is `Missing`, and `@a:x`'s file is byte for byte the same afterwards (same bytes, same modification time);
  - `write_sidebar` of a `@b:x` layout creates `@b:x`'s own file and leaves `@a:x`'s file byte for byte the same; then both read back their own layouts;
  - a file placed at `@b:x`'s path whose `account` is `@a:x` (the collision or planted case), or has no or an invalid `account`, reads as `OtherAccount` for `@b:x`, is not renamed, and `write_sidebar` and `remove_sidebar` for `@b:x` return `Protected` and leave it byte for byte the same;
  - `write_sidebar` and `remove_sidebar` over a `"version":2` file return `Protected` and leave it unchanged.
- Hostile input never panics, finishes in under 1 s, and keeps the valid rest:
  - IDs without `!`, with spaces or non-ASCII characters, or 256 bytes long;
  - labels of 33 characters, empty, all spaces, with `\u{202E}`, `\n` or `\u{200B}`;
  - duplicate spaces in the rail and in folders;
  - one room in two sections;
  - 300 rail entries, 60 folders, 30 sections and 5,000 room references;
  - nesting 200 levels deep;
  - a 300 KiB file, a directory, a FIFO (Unix) and a symlink to a FIFO (Unix).
- Caps: `normalized()` never exceeds a limit. `fits()` is false once the reference count or ID bytes would pass the budget. A test writes the largest layout that fits and checks the file is at most `MAX_SIDEBAR_BYTES`.
- `format!("{:?}", layout)` of a layout holding `!secretroom:x`, the account `@me:x` and the label "Payroll" contains none of them (one test for each type that has `Debug`).
- `the_data_root_holds_only_the_marker_and_the_sessions_dir` still passes: nothing creates a sidebar file by reading. `settings.json`'s 64 KiB behaviour (`an_oversized_settings_file_gives_defaults`) is unchanged.
- The serializer guard test (`session.rs`) still passes, with no new `Serialize` and no `serde_json::to_`. `scripts/check-core-api.sh` passes.
- Mutations, each recorded and reverted: dropping the version check fails the `Newer` test; dropping the account check fails the `OtherAccount` test; dropping the write guard fails the "never overwritten" test; naming the file `sidebar.json` for every account fails the two-accounts test; dropping the ID byte rule fails the hostile test; putting `RoomId` back into the derived `Debug` fails the redaction test.
- The gate passes. The commit is `feat(core): keep each account's sidebar layout in a local, versioned file` with `Step: sbc.1`.

Evidence: `cargo test -p twicecat-core --lib paths:: api::sidebar::` passes. Nothing on screen changes in this task.

### Task 2: The storage boundary and the sidebar model in the app (no UI change)
Language: rust
Files:
- `crates/app/src/sidebar_store.rs` (new): `SidebarStore`, `SidebarLoad`, `DiskSidebar` (coalesced writes, latest wins, under `blocking::run`), `NoSidebarStore` and `MemorySidebar` (`#[cfg(test)]` or `pub` behind `test-support`, following how `NoSettingsStore` is exposed).
- `crates/app/src/sidebar.rs` (new): `SidebarModel` (`account`, `layout`, `state: NotLoaded | Loaded | Newer | OtherAccount | Full`, redacted `Debug`), `load_for(account)`, `apply(Op) -> Result<(), Refused>`, which saves to the model's own account after every successful operation, `unload()` (drops the layout and account from memory, calls nothing on the store) and `reset()` (for Task 13). `Op` holds only the operations of later tasks, introduced here as an enum with no UI caller yet: `ToggleFolder` and `ToggleSection`.
- `crates/app/src/state.rs`: `App.sidebar` and `App.sidebar_store` (`NoSidebarStore` in `App::new`; `App::booter` takes the store). On `LoggedIn`, load for that account; on `LoggedOut` or `SessionExpired`, `unload` only. No sidebar file is removed at logout (Q3).
- `crates/app/src/main.rs`: `DiskSidebar::new(data.clone())` passed to the booter.
- `crates/app/src/lib.rs`, `PROGRESS.md` and the ledger.

Depends on: Task 1
Acceptance criteria:
- With `MemorySidebar`:
  - logging in as `@a:x` loads once for `@a:x`;
  - `Newer` and `OtherAccount` each set their state, and every later `apply` is refused with no save;
  - a successful `apply` saves exactly once, for the logged-in account, with the new layout, and a refused one (`Full`) saves nothing;
  - logout makes no store call at all (no `save`, no `reset`), and `app.sidebar` is back to `NotLoaded` with no layout;
  - logging out of `@a:x` and in as `@b:x` loads for `@b:x` only; an `apply` then saves for `@b:x` only, and no call ever names `@a:x` after the logout;
  - an `apply` that arrives between logout and the next login is refused and saves nothing.
- `DiskSidebar` (tests with a temp `DataPaths`):
  - two saves in quick succession leave the newer layout on disk, and a failed write yields `false` without a panic;
  - **kept at logout:** after `@a:x` saves, logs out and logs back in, the same layout loads, and the file is byte for byte the one written before the logout;
  - **another account's file is ignored and never overwritten:** after `@a:x` saves and logs out, `@b:x` logs in, gets the default layout, applies two operations; `@a:x`'s file is byte for byte unchanged, and `@b:x`'s file holds `@b:x`'s layout;
  - a protected file at the account's path (`account` of another user, or `"version":2`) loads as `OtherAccount` or `Newer`, a forced `save` yields `false`, and the file stays byte for byte the same.
- A source-scan test: `screens/` never names `DiskSidebar`, `paths::`, `read_sidebar` or `write_sidebar`; only `sidebar_store.rs` does. This is the boundary that a later sync implementation relies on.
- `format!("{:?}", app.sidebar)` contains no room ID from the test layout.
- The existing state, gallery and `menus_ignore_the_wheel` tests pass unchanged, so the screen does not change.
- The gate passes. The commit is `feat(app): a storage boundary and model for the sidebar layout` with `Step: sbc.2`.

Evidence: `cargo test -p twicecat-app --lib sidebar` passes. With `cargo run -p twicecat-app -- --data-dir <scratch>`, logging in to the local homeserver creates no `sidebar-*.json`, because nothing has been changed yet.

### Task 3: A space's rooms in `SpaceView` (core, D1)
Language: rust
Files:
- `crates/core/src/api/rooms.rs`: `SpaceView.rooms: Vec<RoomId>`, its documentation (the per-level meaning, the cap) and `MAX_SPACE_ROOMS = 2000`. `Debug` shows `rooms.len()`.
- `crates/core/src/spaces.rs`: `view_of` fills `rooms` from `descendants`, capped, and its tests.
- The CLI's `Spaces` printing (counts only), and the app's test literals of `SpaceView` (`spaces.rs`, `spaces_sidebar.rs`, `ui_gallery.rs`): the field is added.
- `crates/core/tests/spaces.rs` (new, local homeserver): a space with two rooms and a sub-space holding one room. The level-0 view's `rooms` holds the two rooms and the sub-space; the level-1 view's `rooms` holds its room.
- `PROGRESS.md` and the ledger.

Depends on: none (it can run alongside Tasks 1 and 2; it is ordered here to keep the core steps together)
Acceptance criteria:
- The unit tests: `rooms` equals `descendants` up to the cap. 2,005 descendants give 2,000. The `Debug` output has a count and no ID.
- The integration test above passes against the local stack (`scripts/run-integration-tests.sh`), and if level 0 does not hold the sub-space ID, the work stops and asks (rule 8).
- The app compiles and its tests pass with the field unused.
- The gate passes. The commit is `feat(core): say which rooms each space holds` with `Step: sbc.3`.

Evidence: `twicecat-cli` against the local homeserver prints each space with its room count (for example `space level=0 rooms=3`), and no names.

### Task 4: Unread totals per space (core, D2)
Language: rust
Files:
- `crates/core/src/api/rooms.rs` (or `api/types.rs`, next to the room types): `SpaceUnreadView` and `UnreadCountsView`.
- `crates/core/src/api/mod.rs`: `CoreEvent::SpaceUnread { spaces, outside }` and its `Debug`.
- `crates/core/src/notifications.rs`:
  - the pure `space_totals(rooms: &[RoomCounts + id], children: &SpaceChildren) -> (Vec<SpaceUnreadView>, UnreadCountsView)`;
  - `RoomCounts` gains the room ID and an `unread` flag;
  - the runner sends `SpaceUnread` with the same throttle, only when it changes, and reads the `SpaceChildren` watch.
- `crates/core/src/engine/mod.rs`: wiring.
- The CLI: one `space-unread` line with counts only.
- `crates/core/tests/notifications.rs`: a new local-homeserver case.
- `PROGRESS.md` and the ledger.

Depends on: Task 3
Acceptance criteria:
- Unit tests of `space_totals`:
  - a muted room counts nowhere;
  - a room in a level-1 child counts toward both the child and its top-level space;
  - a room that is in two top-level spaces counts in both;
  - rooms in no space and DMs outside spaces count in `outside`;
  - the sums saturate at `u64::MAX`;
  - `unread` is true for `marked_unread` or `unread_messages > 0` alone.
- The runner sends nothing when nothing changed and at most once per interval (the existing `run_totals` tests, extended).
- Integration (local homeserver): user B posts in a room of user A's space; A receives `SpaceUnread` with that space's `notifications ≥ 1`; after A marks it read, the count returns to 0.
- `call_log_hygiene.rs` and `no_secret_logging.rs` still pass. The CLI line holds no ID and no name.
- The gate passes. The commit is `feat(core): roll unread counts up to each space` with `Step: sbc.4`.

Evidence: `twicecat-cli` prints `space-unread spaces=2 outside=(n=1,h=0)` after a message from the second test account.

### Task 5: The rail's look (RailStates) and its badges
Language: rust
Files:
- `crates/app/src/spaces.rs`:
  - `SpacesModel` keeps `SpaceUnread` (`unread_for(id)`, `outside()`);
  - `entries()` lists top-level spaces only (decision 3, Q5);
  - `rail_items(layout)` merges the layout's order and folders with the core's spaces: layout order first, new spaces appended in core order, unknown ones skipped.
- `crates/app/src/screens/spaces_sidebar.rs`:
  - `SIDEBAR_WIDTH = 72`;
  - a `mouse_area` per item (hover, click; no button inside);
  - the pill, circle or squircle and ring, the badge, the tooltip `.delay(500 ms)`;
  - the folder icon (2 × 2 preview) and the open-folder band;
  - the pure helpers `pill_height(state)`, `icon_radius(state)` and `badge_text(counts)`.
- `crates/app/src/state.rs`: `rail_hover`; handles `CoreEvent::SpaceUnread`; `ToggleFolder` through `SidebarModel`.
- `crates/app/src/screens/main_view.rs`: the width constant only.
- `crates/app/tests/ui_gallery.rs`:
  - the scenario `rail-states`: the five states side by side, Home with a badge, a collapsed folder, an open folder;
  - the existing scenarios redrawn at the new width.
- `PROGRESS.md` and the ledger.

Depends on: Task 2, Task 4
Acceptance criteria:
- Pure tests:
  - `pill_height`: 0, 8, 20 or 32 for default, unread, hover and selected (selected wins over hover, and hover over unread);
  - `icon_radius`: 22 idle, 14 for hover and selected;
  - `badge_text`: highlight first, "99+" above 99, nothing for unread alone;
  - a folder's counts are the sum of its spaces' counts.
- `rail_items`:
  - the layout order is kept;
  - a new space is appended;
  - a space in a folder is not repeated at top level;
  - a stored space the core does not report is not drawn but stays in the layout.
- State: `SpaceUnread` updates the badges; a click (press and release with no movement) selects the space, as `SpacesMessage::Select` did; a click on a folder toggles it and saves once.
- Gallery: `rail-states` draws with no overlap at 1280 and 720 wide. The layout checks of every existing scenario pass at the new width. The badge colours are at least ΔE 20 from the rail background in every test theme (`theme::test_themes()`).
- Mutations, each recorded and reverted: hover winning over selected fails the pill test; dropping `outside` from Home's badge fails the state test.
- The gate passes. The commit is `feat(app): space rail with unread pills, mention badges and folder previews` with `Step: sbc.5`.

Evidence: `TWICECAT_GALLERY_DIR=target/ui-gallery TWICECAT_GALLERY_ONLY=rail- cargo test -p twicecat-app --test ui_gallery -- --ignored` writes `rail-states-*.png`. With `cargo run -p twicecat-app -- --data-dir <scratch>` on the local homeserver, a message from the second test account lights its space's pill.

### Task 6: Arranging the rail with the options menu (folders included)
Language: rust
Files:
- `crates/app/src/sidebar.rs`: `Op::{MoveSpace { id, dir }, MoveToFolder { id, folder: FolderTarget::{Existing(u16), New(SidebarLabel?)} }, RemoveFromFolder, RenameFolder, UngroupFolder, MoveFolder}`. A folder that holds fewer than 2 spaces dissolves. Folder IDs are the lowest free `u16`.
- `crates/app/src/screens/sidebar_menu.rs` (new): the options card for rail items and the name dialog (`kit::dialog`, `text_input`, `SidebarLabel` validation, an error line).
- `crates/app/src/state.rs`: right-press opens the card; Esc order; messages.
- `crates/app/tests/ui_gallery.rs`: the scenarios `rail-menu` (the card open on a space) and `rail-folder-name` (the dialog with an invalid name and its error line).
- `PROGRESS.md` (including the keyboard-only limit in its open issues) and the ledger.
- `crates/app/src/keys.rs` is **not** changed (Q12).

Depends on: Task 5
Acceptance criteria:
- Operation tests, each followed by one save:
  - Move up on the first item and Move down on the last change nothing and save nothing;
  - moving a space into a new folder creates the folder at the space's place;
  - moving the second-to-last space out of a folder dissolves it;
  - Ungroup puts the spaces back in their order;
  - an edit that would pass `MAX_FOLDERS` is refused, with "The sidebar layout is full";
  - in the `Newer` state every operation is refused.
- Menu: Move down on space B moves B down one item and saves. Inside an open folder it moves B within the folder; Move down on a collapsed folder's own card moves the folder. Every rail operation above is reachable from a card (a table test from each card's entries to `Op`s).
- No chord: the commit does not touch `keys.rs`, `SHORTCUTS` is unchanged, and the existing `keys.rs` and Settings keyboard-list tests pass unchanged.
- The name dialog takes the focus on open. Enter with "  Work  " saves "Work"; a 33-character name or `"\u{202E}x"` shows the error and saves nothing; Esc cancels.
- `menus_ignore_the_wheel.rs` covers the folder pick list. The Esc order test gains the options card and the dialog.
- Gallery: the new scenarios pass the layout checks at both widths.
- Mutations, each recorded and reverted: not dissolving single-space folders fails its test; saving on a no-op move fails the "saves nothing" test.
- The gate passes. The commit is `feat(app): reorder spaces and group them into folders from an options menu` with `Step: sbc.6`.

Evidence: in a running app on the local homeserver, right-click a space → Move to folder → New folder "Work" → the folder appears with a 2 × 2 preview. `<scratch>/sidebar-<digest>.json` has a `folder` entry and no space names. Right-click → Move up moves the space.

### Task 7: Dragging on the rail
Language: rust
Files:
- `crates/app/src/drag.rs` (new): the state machine of decision 6, generic over the item and target types, with no iced types besides `Point`.
- `crates/app/src/screens/spaces_sidebar.rs`: `on_press`, `on_move`, `on_enter` and `on_exit` on items; `interaction(Grab/Grabbing)`; the target line or ring.
- `crates/app/src/state.rs`: the window listener for left release and focus loss during a drag (in `App::subscription`, active only while a drag runs); Esc cancels.
- `crates/app/tests/ui_gallery.rs`: the scenario `rail-dragging` (a drag in progress with the "before" line, and one with the "into folder" ring).
- `PROGRESS.md` and the ledger.

Depends on: Task 6
Acceptance criteria:
- State-machine tests:
  - a movement under 4 px followed by a release is a click;
  - 4 px or more starts a drag;
  - the top, middle and bottom thirds give before, into and after;
  - a release with no target, Esc or focus loss cancels and saves nothing;
  - a second press during a drag is ignored.
- Every drop maps to an `Op` that Task 6's menu also produces: a table test over all target kinds checks the same layout results from the drag and from the menu.
- Dropping a space onto itself, or a folder into a folder, is a no-op (no save).
- The subscription listens only while a drag runs (as in `ptt.rs`'s subscription test).
- Mutation, recorded and reverted: dropping the 4 px threshold fails the click test.
- The gate passes. The commit is `feat(app): drag spaces to reorder them or to make folders` with `Step: sbc.7`.

Evidence: the gallery writes `rail-dragging-*.png`. In a running app, dragging a space onto another makes a folder. The feel of the drag is on the human checkpoint.

### Task 8: Sections inside a space (model, variable-height windowing, view)
Language: rust
Files:
- `crates/app/src/sections.rs` (new):
  - `build(space, spaces, rooms, layout, calls, collapsed) -> Vec<Line>`, which places rooms and orders sections as in decision 4;
  - `section_badge(rooms) -> Badge`;
  - `line_height(line, density)`, `line_offsets` and `visible_lines` (binary search; NaN and stale offsets clamped as `visible_range` does);
  - the load-all rule.
- `crates/app/src/room_list.rs`: shares the clamping helpers; `visible_range` is kept for the flat list.
- `crates/app/src/screens/room_list_pane.rs`: in sectioned mode, `windowed_list` builds `Line`s (header rows with a chevron, the name, "personal" on Starred and the rolled-up badge when collapsed; the existing room rows); a header click sends `ToggleSection`.
- `crates/app/src/state.rs`: `ToggleSection` through `SidebarModel`; Alt+Up/Down and Ctrl+Shift+Up/Down walk the drawn room lines; `LoadMoreRooms` while sectioned and not complete.
- `crates/app/tests/ui_gallery.rs`: the scenarios `space-sections` (Starred, Channels, Tech and Voice sub-spaces, Direct messages; Comfortable and Compact) and `space-sections-collapsed` (Tech collapsed with a highlight badge, Hobbies with a count).
- `PROGRESS.md` and the ledger.

Depends on: Task 3, Task 5
Acceptance criteria:
- Placement tests (fixtures of made-up IDs):
  - every room of the space appears exactly once;
  - Starred wins over a custom section, a custom section over a sub-space, and a sub-space over DM or Channels;
  - sub-space rooms (`is_space`) are not rows;
  - a level-2 room lands in its level-1 section;
  - empty built-in sections are hidden, and an empty custom section shows its hint line;
  - the stored `order` is kept, and missing keys go to their defaults.
- Badge tests: a collapsed section with highlights 2 and notifications 5 shows `Highlight(2)`; with only `marked_unread`, `Dot`; an expanded section shows no header badge.
- Windowing tests: `visible_lines` over mixed heights returns exactly the lines that cross the viewport plus overscan; the sum of the spacers and the built heights equals the total; NaN or a negative offset gives the first page.
- When sections show: only with a space selected, the All tab, a blank search and the expanded list. A search, the People tab or the rail gives the flat list exactly as before (the existing room-list tests pass unchanged).
- Keyboard: Alt+Down from the last room of an expanded section goes to the first room of the next drawn section and skips collapsed ones.
- Load-all: in sectioned mode with `total > len`, `LoadMoreRooms` is sent once per loaded page until `len == total`.
- Gallery: both scenarios pass the layout and overlap checks at 1280 and 720 wide, in both densities and every test theme.
- Mutations, each recorded and reverted: letting a room appear in two sections fails the placement test; an off-by-one in `visible_lines` fails the windowing test.
- The gate passes. The commit is `feat(app): room list sections inside a space with rolled-up counts` with `Step: sbc.8`.

Evidence: `TWICECAT_GALLERY_ONLY=space-sections` writes the two scenario PNG images. In a running app on the local homeserver, selecting a test space with a sub-space shows the Channels and sub-space sections; collapsing one survives a restart and a logout and login of the same account (`sidebar-<digest>.json` has its `collapsed` key).

### Task 9: User-defined sections, Starred, and moving rooms by the options menu
Language: rust
Files:
- `crates/app/src/sidebar.rs`: `Op::{Star, Unstar, NewSection, RenameSection, DeleteSection, MoveSection { dir }, MoveRoomToSection, MoveRoomInSection { dir }, RoomToDefault}`.
- `crates/app/src/screens/sidebar_menu.rs`: header and row options cards; the "⋯" on headers and on the selected row.
- `crates/app/src/state.rs`: messages, Esc order.
- `crates/app/tests/ui_gallery.rs`: the scenarios `space-sections-custom` (two custom sections, one empty with its hint) and `section-menu`.
- `PROGRESS.md` and the ledger.
- `crates/app/src/keys.rs` is **not** changed (Q12).

Depends on: Task 6, Task 8
Acceptance criteria:
- Operation tests:
  - Star puts the room in Starred and Unstar returns it to its default section;
  - New section "Ops" appears in its default place and is saved;
  - Rename keeps the rooms;
  - Delete returns its rooms to their default sections and drops its key from `order` and `collapsed`;
  - Move room to section takes it out of any other custom section (one place per space);
  - Move up/down inside a custom section changes only that section's order;
  - Move section up/down swaps with the drawn neighbour, hidden sections skipped;
  - passing `MAX_CUSTOM_SECTIONS` or the room budget is refused.
- Menu: Star on a room's card toggles its star and saves once; Move down on a room in a custom section moves it within that section; Move down on a header's card moves the section. Every section and room operation above is reachable from a card, from right-click and from the "⋯" (a table test from each card's entries to `Op`s).
- No chord: the commit does not touch `keys.rs`, `SHORTCUTS` is unchanged, and the existing `keys.rs` and Settings keyboard-list tests pass unchanged.
- `menus_ignore_the_wheel.rs` covers the "Move to section" pick list.
- Gallery: the scenarios pass the layout checks.
- Mutation, recorded and reverted: deleting a section without returning its rooms fails the delete test.
- The gate passes. The commit is `feat(app): your own room sections, Starred, and moving rooms between them` with `Step: sbc.9`.

Evidence: in a running app, right-click a room → Move to section → New section "Ops"; the room appears under "Ops"; after a restart it is still there; `sidebar-<digest>.json` holds `"custom"` with the label and the room ID, and no room name.

### Task 10: Dragging rooms and sections
Language: rust
Files:
- `crates/app/src/screens/room_list_pane.rs`: `mouse_area` on room rows and headers in sectioned mode (rows keep their click-to-open; a click remains press and release with no movement), the drop line and the header highlight.
- `crates/app/src/state.rs`: room-list targets in the drag machine; drops call Task 9's operations.
- `crates/app/tests/ui_gallery.rs`: the scenario `space-sections-dragging`.
- `PROGRESS.md` and the ledger.

Depends on: Task 7, Task 9
Acceptance criteria:
- A table test checks that each room-list drop (onto a header, between rows of a custom section, header onto header) gives the same layout as the matching menu operation.
- Dropping into a sub-space, Channels or Direct messages header means "back to its default section", and only into the room's own default section: elsewhere the target is refused and drawn as not allowed.
- A row click still opens the room (the existing selection tests pass), and the right-press card still opens.
- In flat mode no row is draggable.
- The gate passes. The commit is `feat(app): drag rooms between sections and reorder sections` with `Step: sbc.10`.

Evidence: the gallery writes `space-sections-dragging-*.png`. In a running app, dragging a room onto "Ops" moves it there.

### Task 11: Who is in each voice room (core, D3)
Language: rust
Files:
- `crates/core/src/api/call.rs`: `CallMemberView`, with a redacted `Debug`.
- `crates/core/src/api/mod.rs`: `CoreEvent::ActiveCall` gains `members`.
- `crates/core/src/call/watcher.rs`: `on_members` builds the distinct members (cap 25, join order), resolves names and avatars from the room's member store (`clean_line`, `pick_avatar`), and sends when the set or the count changes. Tests.
- The CLI's `ActiveCall` line (counts only); the app's `calls.rs` match arms and tests (the field is ignored until Task 12).
- `crates/core/tests/call_signalling.rs` (or a new local-homeserver case): B joins A's voice room; A receives `ActiveCall` with B in `members`; B leaves and A receives an empty set.
- `PROGRESS.md` and the ledger.

Depends on: none (ordered here before its first user)
Acceptance criteria:
- Unit tests:
  - the same user on two devices gives one member and `participants == 2`;
  - 30 users give 25 members;
  - a name with `\u{202E}` and a newline is cleaned;
  - an invalid avatar MXC is dropped;
  - a change of the member set with an unchanged count still sends an event.
- `Debug` of `CallMemberView` and of `ActiveCall` shows user IDs as the other call events do, but never display names (the same rule as `Participant`).
- The integration case passes against the local stack. `call_log_hygiene.rs` passes.
- The gate passes. The commit is `feat(core): say who is in each room's call` with `Step: sbc.11`.

Evidence: `twicecat-cli` with two test accounts: when the second joins a voice room, the first prints `active-call participants=1 members=1`.

### Task 12: Voice channels with their people in the sidebar
Language: rust
Files:
- `crates/app/src/calls.rs`: `members: HashMap<RoomId, Vec<CallMemberView>>` (cleared with the counts); `member_marks(room, user) -> (speaking, muted)` from `CallParticipants`, for this device's call only.
- `crates/app/src/sections.rs`: `Participant` and `More(n)` lines under voice rooms (at most 8, then "and N more"), hidden with a collapsed section.
- `crates/app/src/screens/room_list_pane.rs`: participant lines (20 px avatar, cleaned name or localpart, a `speaking` ring, the muted icon); a click selects the voice room. Avatars are requested once.
- `crates/app/tests/ui_gallery.rs`: the scenario `voice-channel`, as in board 6: the voice section with "vocal" holding three members, one speaking and one muted, while this device is in the call, plus a second voice room with a member and no marks.
- `PROGRESS.md` and the ledger.

Depends on: Task 8, Task 11
Acceptance criteria:
- Tests:
  - members are drawn under their room, in order;
  - the ninth member onward becomes "and N more";
  - speaking and muted come only from this device's call, and members of another room never show them;
  - a member with no name shows the localpart, and a name with bidi characters is cleaned;
  - leaving the call removes the marks at once;
  - `ActiveCall` with zero members removes the lines.
- The speaking ring is at least 3:1 against the row background in every test theme (the kit's non-text rule).
- The flat list (search, other tabs) still shows the current call pill and no participant lines.
- Gallery: `voice-channel` passes the layout checks at both widths.
- Mutation, recorded and reverted: showing `speaking` for a room that is not this device's call fails the marks test.
- The gate passes. The commit is `feat(app): voice channels list who is in them, who speaks and who is muted` with `Step: sbc.12`.

Evidence: the gallery writes `voice-channel-*.png`. On the local stack with two test accounts both in a voice room, the sidebar lists both. The speaking ring follows the voice, which is a human checkpoint item.

### Task 13: Reset, documentation and wrap-up
Language: rust
Files:
- `crates/app/src/screens/settings.rs`: Settings → Appearance → "Reset sidebar layout..." with a confirmation (calls `SidebarModel::reset`, which calls `SidebarStore::reset` for the logged-in account only). In the `Newer` and `OtherAccount` states the button is turned off and the matching note is shown there.
- `docs/accessibility.md`: a sidebar paragraph. It says the sidebar adds no keyboard chord; that every move, folder move, section move and star is in the right-click / "⋯" options menus, the alternative to dragging; that those menus need a pointer, because iced 0.14 buttons take no focus, so keyboard-only users keep the arrangement but cannot change it (decision 5, "Keyboard-only users"); and the no-auto-scroll limit. The keyboard table is unchanged.
- `docs/sidebar.md` (new, user docs):
  - what can be arranged, and that it is done through the options menus or by dragging;
  - that the layout is kept **on this device only**, one file per account;
  - where the files live (`sidebar-<digest>.json` next to `settings.json`); that a file is **kept at logout** and restored at the next login of the same account; that another account on the device never reads or changes it; and how to remove it (Reset while logged in as that account, or delete the file);
  - the caps;
  - what "made by a newer Twicecat" and "belongs to another account" mean.
- `PROGRESS.md`: the checkpoint list and the open issues (the keyboard-only limit). The ledger.

Depends on: Task 12
Acceptance criteria:
- Reset after confirming clears the layout, removes the logged-in account's file, and shows the default rail and sections. Cancel changes nothing. In a temp `DataPaths` test with files for `@a:x` and `@b:x`, Reset as `@b:x` removes only `@b:x`'s file, and `@a:x`'s file is byte for byte the same.
- In the `Newer` and `OtherAccount` states the Reset button is off, and the protected file stays byte for byte the same.
- The existing docs test that the keyboard table in `docs/accessibility.md` matches `SHORTCUTS` passes unchanged: no row is added.
- `docs/accessibility.md` names the options menus and states that they need a pointer (a grep test).
- `docs/sidebar.md` mentions `sidebar-`, "this device only", that the file is kept at logout, and that other accounts do not read or change it. It does not claim a sync and does not say the file is removed at logout (a grep test).
- The gate passes. The commit is `feat(app): reset the sidebar layout, and document the customizable sidebar` with `Step: sbc.13`. It is a `feat` because it adds the Reset button.

Evidence: Settings → Appearance → Reset sidebar layout → confirm: the folders and custom sections disappear, the logged-in account's `<scratch>/sidebar-<digest>.json` is gone, and another test account's sidebar file in the same folder is still there, unchanged.

## Human checkpoint (added to PROGRESS.md "Sidebar customization human checkpoint" by the task named)
- (Task 5) The rail in a real window on Linux (Wayland and X11), Windows and macOS:
  - the circle-to-squircle change on hover;
  - the pill heights;
  - the badge outline;
  - the tooltip delay;
  - the 2 × 2 folder preview;
  - Home's badge;
  - both densities and every shipped theme (Twicecat and Daylight).
- (Task 5) Sub-spaces no longer appear on the rail (Q5): is the navigation acceptable?
- (Tasks 7 and 10) Dragging feels right:
  - the 4 px threshold;
  - the cursor changes;
  - the target line and ring;
  - Esc and alt-tab cancel;
  - no stuck drag after releasing outside the window;
  - a target out of view can be reached through the menu (no auto-scroll).
- (Tasks 6 and 9) Every action is possible without dragging, through the right-click and "⋯" options menus, on each OS, including a trackpad's secondary click on macOS (whether Ctrl-click reaches `on_right_press` is checked there too). No new keyboard chord exists, and the existing shortcuts behave as before.
- (Tasks 6, 9 and 13) **Known limit, for the human to accept:** keyboard-only users cannot open the options menus, because iced 0.14 buttons take no focus and the user chose no chords (Q12). They keep the default or an earlier arrangement, Ctrl+K and the room-stepping keys, but cannot rearrange the sidebar or expand a collapsed section. Is this acceptable for now? It is also listed in PROGRESS.md's open issues.
- (Task 8) Sections in a real space:
  - collapsed badges;
  - the "personal" label on Starred;
  - the scroll position kept when a section collapses;
  - a large space (500+ rooms) scrolls smoothly.
- (Task 12) In a real call:
  - the speaking ring follows the voice with no visible lag;
  - mute shows at once;
  - members of other voice rooms appear and leave within a few seconds.
- (Tasks 2 and 13) With two local test accounts in a real app:
  - logging out keeps the first account's sidebar file;
  - logging in as the second account starts with the default layout, and its changes go to its own file;
  - the first account's file is untouched (same bytes);
  - logging back in as the first account restores its layout;
  - Reset removes only the logged-in account's file.

What cannot be verified headlessly:
- real rendering (the gallery uses tiny-skia, not wgpu);
- overlays: tooltips, pick lists and the options card;
- pointer drags, hover and right-click;
- live voice activity.

## Questions
Q1 to Q13 are resolved by the user's answers. Q14 stays open as a note for a later sync plan; nothing in it is planned here.

- **Q1 (branch): resolved.** Keep working on the same branch, `feat/ui-ux-rework`. No new branch is created, and opening a PR is not a step of this plan (decision 0).
- **Q2 (file): resolved, yes.** The layout lives in a sidebar file of its own next to `settings.json`, never inside `settings.json`. With Q3 it is one file per account, `sidebar-<digest>.json` (decision 1).
- **Q3 (logout): resolved, keep the file, bound to the account.** The file is **not** removed at logout. Each account has its own file, named by an FNV-1a digest of its user ID, and the file's `account` field must match the logged-in user. Another account logging in reads its own (missing) file, starts from the default layout and writes only its own file. It never reads, writes, renames or removes the first account's file. A file at an account's path whose `account` does not match is `OtherAccount`: ignored, never overwritten, customizing off with a note. The budget and validation rules are unchanged (decision 1; Tasks 1, 2 and 13).
- **Q4 (newer files): resolved, yes.** A file with a higher `version` is left untouched (not renamed, not overwritten, not removed by Reset), and customizing is turned off with a note.
- **Q5 (sub-spaces): resolved, yes.** Sub-spaces leave the rail and become room-list sections. Level-2 sub-spaces are flattened into their level-1 section.
- **Q6 (room order inside sections): resolved, yes.** Rooms inside a section stay ordered by recent activity. No core task reads the `m.space.child` order of children.
- **Q7 (mention badge colour): resolved, yes.** Mention badges use the theme's `highlight` colour.
- **Q8 (Direct messages in a space): resolved, yes.** The Direct messages section shows only DMs that are children of the space. No core change.
- **Q9 (folders): resolved, yes.** A folder with fewer than 2 spaces dissolves. Folders have an optional name (the tooltip) and no colour.
- **Q10 (Starred): resolved.** Starred is local and per space only. Rooms with the Matrix `m.favourite` tag are **not** added, and the core gains no favourite field.
- **Q11 (row look): resolved, yes.** Rows inside sections keep today's room row. No restyle.
- **Q12 (keyboard chords): resolved, no chords at all.** Moves, folder moves, section moves and starring are available only through the right-click / "⋯" options menus (and dragging). No `SHORTCUTS` row is added, and existing shortcuts are untouched. The keyboard-only limit that follows (menus need a pointer, because iced 0.14 buttons take no focus) is stated in decision 5, in the docs, in PROGRESS.md's open issues and on the human checkpoint.
- **Q13 (voice marks): resolved, yes.** Speaking and muted are shown only for the call this device is in. Other voice rooms show who is there.
- **Q14 (later Matrix account-data sync): open, not planned.** It is listed as the user asked. Before any sync work, the human must approve a schema (AGENTS rule 8: a private format with no MSC):
  - **Privacy:** account data is **not end-to-end encrypted**. The homeserver, and its admins, would see the room IDs, the user's folder and section names, and how the user groups their rooms. Should the names be left out, or the whole blob encrypted (for example with a key from the user's secret storage)? That is extra crypto work.
  - **Conflicts:** account data is last-write-wins per event type. Two devices that arrange the sidebar at the same time lose one side's changes, unless the layout is split into several events (one per space) or merged field by field with timestamps.
  - **Size:** homeservers limit event size (65,536 bytes per event in the spec, smaller in practice once encoded). The local 192 KiB budget would not fit in one event, so a split or tighter caps would be needed.
  - **Stale rooms:** rooms that were left, and rooms another device has not synced yet, will appear in a synced layout. A rule is needed for when entries are pruned and by which device.
  - **Schema:** a private event type (for example `io.github.ga_lep.twicecat.sidebar`) with a `version` field from the first write. Older clients must ignore newer versions without overwriting them (the local `Newer` rule). This needs the human's approval, and the type name is a choice to make.
  - **Interop:** Element and other clients use `m.space_order` (MSC3230) for top-level space order. Should a synced rail order also write it? It is visible to other clients and has its own semantics.
  - **Migration:** on first sync, which wins between the local file and the account data, and is the user asked?

PLAN READY — awaiting validation
