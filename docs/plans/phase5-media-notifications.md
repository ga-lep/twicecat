# Phase 5 Media, Notifications and Rich Features Plan
Status: APPROVED

## Goal
Phase 5 adds what makes Tessera feel complete: images and files in the timeline (thumbnails, a full-size viewer, saving to disk), uploads (file picker, drag and drop, clipboard paste, thumbnails generated before sending, upload progress), desktop notifications driven by the SDK's push-rule evaluation and the room notification settings, a tray icon with an unread badge, room management (create, including encrypted rooms, join by alias, invite, leave, room settings, member list with power levels), spaces as a sidebar filter, threads in a side panel with their own `Timeline` focus, a Settings screen (profile, notifications, theme, keyboard shortcuts, a push-to-talk placeholder), and an accessibility pass (Ctrl+K room switcher, arrow keys in lists, documented screen-reader gap). It also closes the items earlier phases deferred here: mention pills labelled from their target (Phase 3 Q20), joining rooms from pills (Phase 3 Q24), creating encrypted rooms from the UI (Phase 4 Q17), and the deferred IME check (Phase 3) goes on this checkpoint.

The core (behind the additive command/event API, Phase 2 Q6 convention) gains:
- **Media:** media messages become `Message` items with a `MediaContent` description; the app fetches thumbnails and full files by timeline item (never by MXC URI or encryption key), and the core saves a file to a path the user picked. The SDK decrypts encrypted media.
- **Uploads:** `SendAttachment` through the SDK send queue, with local echo, upload progress, a caller-supplied thumbnail and the server's upload limit.
- **Notifications:** the SDK's notification handler (push rules evaluated on every sync) feeds a `Notification` event. Room modes, default modes and keywords are read and written through `NotificationSettings`. Unread totals for the tray badge are computed over all rooms.
- **Rooms and spaces:** create (plain, encrypted, DM), join by ID or alias, accept and decline invites, invite, leave, room settings, members with power levels, the user's own profile, and space filters for the room list.
- **Threads:** thread summaries on thread roots, threaded replies hidden from the main timeline, and a second, thread-focused timeline.

The app gains: inline thumbnails with fixed boxes (no scroll jumps), a full-size viewer, file/video/audio cards, an upload flow, desktop notifications and a tray icon, room dialogs and a room settings panel, a member list, the spaces sidebar, the thread panel, the Settings screen, the room switcher and keyboard navigation.

Rendering, OS integration (dialogs, clipboard, notifications, tray, drag and drop), keyboard-only use and IME stay on the human checkpoint (rule 9).

### Human answers recorded
The human's answers of 2026-10-08, as relayed by the controller:
- **Q3: accepted, with the latest versions.** Keep `notify-rust =4.18.2` and `tray-icon =0.26.1` (not the older 4.18.1/0.26.0), and keep the scoped `ksni` Unlicense exception in `deny.toml`.
- **Q4: accepted.** No drag and drop on a native Wayland window; the gap is recorded as planned.
- **Q5: accepted.**
- **Q2: accepted.** Linux-only human checks (Wayland and X11), no CI gate. Pushing the branch so CI builds Windows/macOS is **not** a requirement of any task or of the checkpoint; it is optional at most.
- **Q16: accepted.**
- **Q1, Q6-Q15, Q17-Q21: accepted as recommended.** The human gave no specific answer on these, approved the plan, and said to use the RECOMMENDED option for them and for any later question.
- **Questions during implementation:** the human is unavailable and said to apply the recommendation and keep going. A question that AGENTS.md rule 8 would require stopping for is still logged in PROGRESS.md (open issues and the Phase 5 human checkpoint list) for the human, together with the recommendation that was applied.

### Facts verified while planning (2026-10-08)
Registry root: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Each implementing task re-confirms the APIs it uses (AGENTS.md rule 2) and cites file:line in PROGRESS.md. No code was compiled for this plan except lock and licence probes, run in a scratch copy of the workspace (`git archive HEAD`) in the session scratchpad, not in the repo.

**Precondition (rule 1)**
- Phase 4 is signed off ("Phase 4 signed off by the human on 2026-10-07", PROGRESS.md) and merged: `5d2f4fb Merge pull request #7 from ga-lep/feat/phase4-e2ee`, with `4e33fe2 ledger: phase4-e2ee signed off by the human`. Work happens on `feat/phase5-media-notifications`.
- The Phase 4 checkpoint removed the CI requirement and the Windows/macOS per-OS checks (human decisions, 2026-10-07). Q2 asks whether Phase 5 follows the same rule.

**Media download (matrix-sdk 0.19.1 `src/media.rs`, matrix-sdk-base `src/media/mod.rs`)**
- `Media::get_media_content(&MediaRequestParameters, use_cache)` :431. It decrypts `MediaSource::Encrypted` with `AttachmentDecryptor` (`DefaultMediaFetcher` :805-870). Two limits matter:
  - the whole body is read into a `Vec<u8>`;
  - the request timeout is `Duration::MAX`.
  So the core must enforce its own deadline. Size can only be capped by refusing to fetch what `info.size` already says is too big.
- `get_file(&impl MediaEventContent, use_cache)` :522 and `get_thumbnail(.., MediaThumbnailSettings, use_cache)` :575 (returns `Ok(None)` without a `thumbnail_source`).
- `is_local_uri` :792 serves send-queue local echoes (`mxc://send-queue.localhost/..`) from the media store, so a local echo's media can be fetched before upload completes.
- `get_media_file` :356 writes the decrypted content to a **clear temp file**. Phase 5 does not use it (Q8).
- With `use_cache = true` the content goes to the SQLite media store, which encrypts values with the store cipher (matrix-sdk-sqlite 0.19.1 `media_store.rs:160-177`, `encode_value` :498). Decrypted media is therefore encrypted at rest under the existing passphrase.
- `MediaEventContent` (base media/mod.rs:155) is implemented for the image, video, audio, file, sticker and location contents (:167-223). `MediaThumbnailSettings { method, width, height, animated }` :65.
- ruma-events 0.35.0:
  - `ImageMessageEventContent { body, formatted, filename, source, info }` image.rs:13-39, with `filename()` :72 and `caption()` :80 (a caption exists when `body` differs from the file name);
  - `FileInfo { mimetype, size, thumbnail_info, thumbnail_source }` file.rs:96-115;
  - video and audio have the same shape.
- The core maps image, video, audio and file messages to `Placeholder { what: Image | Video | Audio | File }` today (`crates/core/src/timeline.rs:111-114`).

**Upload (matrix-sdk-ui 0.19.1 `src/timeline/`, matrix-sdk `src/attachment.rs`, `src/send_queue/`)**
- `Timeline::send_attachment(source: impl Into<AttachmentSource>, mime_type: Mime, config: AttachmentConfig) -> SendAttachment` mod.rs:691. `AttachmentSource::{Data { bytes, filename }, File(PathBuf)}` :1059. The `File` variant does a blocking `fs::read` inside the future (:1075-1090).
- `SendAttachment::use_send_queue()` (futures.rs, "(Experimental) ... provides local echoes for the uploaded media too ... will be the default") and `subscribe_to_send_progress()`. With the send queue, `RoomSendQueue::send_attachment` (send_queue/upload.rs:199) refuses rooms that are not joined.
- Progress: `SendQueue::enable_upload_progress(bool)` send_queue/mod.rs:308, off by default (:431). The timeline then reports `EventSendState::NotSentYet { progress: Option<MediaUploadProgress { index, progress: AbstractProgress { current, total } }> }` (ui timeline/event_item/local.rs:67-107; controller/mod.rs:1405-1409).
- `AttachmentConfig` attachment.rs:182 has `txn_id, info, thumbnail, caption, mentions, reply, extra_content`. Related types:
  - `Thumbnail { data, content_type: Mime, height, width, size }` :154;
  - `AttachmentInfo::{Image(BaseImageInfo), Video, Audio, File, Voice}` :84;
  - `BaseImageInfo { height, width, size, blurhash, is_animated }` :34.
- **No thumbnail generation in the SDK:** matrix-sdk 0.19.1 has no `image-proc`-style feature (Cargo.toml `[features]` read in full). Thumbnails must be made by Tessera (Q9).
- `mime` is not re-exported by matrix-sdk or matrix-sdk-ui. matrix-sdk depends on `mime 0.3.17` (Cargo.toml:291), which is already in Cargo.lock.
- `Client::load_or_fetch_max_upload_size() -> Result<UInt>` client/mod.rs:3690.

**Notifications (matrix-sdk `client/mod.rs`, `sync.rs`, `notification_settings/`, matrix-sdk-base `response_processors/`)**
- `Client::register_notification_handler(Fn(Notification, Room, Client) -> Fut)` client/mod.rs:1380. Handlers run after every processed sync response (sync.rs:279-296), and sliding sync goes through the same path (`sliding_sync/client.rs:152` → `call_sync_response_handlers`).
- `Notification { actions: Vec<Action>, event: RawAnySyncOrStrippedTimelineEvent }` (base sync.rs:409). Its `Debug` prints the raw event, so it must never be logged.
- Push rules are evaluated on each timeline event **after** the base client tries to decrypt it (response_processors/timeline.rs:100-145, predicate `Action::should_notify`). Two consequences:
  - an event whose key arrives later is evaluated as `m.room.encrypted` (the default `.m.rule.encrypted`/`encrypted_room_one_to_one` rules notify, but mention and keyword rules cannot match);
  - own events are not filtered by the SDK.
- Invites arrive as stripped events, which is why the `event` field covers both kinds.
- `Client::notification_settings()` client/mod.rs:3572 → `NotificationSettings`. Its methods (mod.rs):
  - `subscribe_to_changes` :124;
  - `get_user_defined_room_notification_mode` :129 and `get_default_room_notification_mode(IsEncrypted, IsOneToOne)` :143;
  - `set_default_room_notification_mode` :198, `set_room_notification_mode` :305, `delete_user_defined_room_rules` :358, `unmute_room` :383;
  - `enabled_keywords` :417, `add_keyword` :426, `remove_keyword` :459.
- `RoomNotificationMode::{AllMessages, MentionsAndKeywordsOnly, Mute}` (base notification_settings.rs:21).
- `Room::notification_mode()` room/mod.rs:3835 and `user_defined_notification_mode()` :3875 cache their result. `cached_user_defined_notification_mode()` (base room/mod.rs:455) is sync, so `summarize` can read it.

**Rooms, members, profile (matrix-sdk `client/mod.rs`, `room/mod.rs`, `account.rs`)**
- `Client` methods:
  - `create_room(create_room::v3::Request)` :1931 (fields `initial_state`, `invite`, `is_direct`, `name`, `preset`, `room_alias_name`, `topic`, `visibility`, `creation_content`: ruma-client-api 0.25.0 room/create_room.rs:54-109);
  - `create_dm(&UserId)` :1964 (encrypted with `e2e-encryption`, Phase 4 fact);
  - `join_room_by_id` :1819, `join_room_by_id_or_alias(&RoomOrAliasId, &[OwnedServerName])` :1840, `resolve_room_alias` :1526.
- `Room` methods:
  - membership: `leave` :471, `join` (accept an invite) :520, `forget` :3793;
  - members: `members(RoomMemberships)` :1233, `members_no_sync` :1246, `get_member` :1199, `invite_user_by_id` :2033;
  - settings: `enable_encryption` :2354, `set_name` :3000, `set_room_topic` :3005, `upload_avatar(&Mime, Vec<u8>, Option<ImageInfo>)` :3040, `remove_avatar` :3029;
  - power levels: `users_with_power_levels` :2988 and `get_suggested_user_role` :2972. `RoomMember::power_level() -> UserPowerLevel` (base room/members.rs:402; `Infinite` for room-v12 creators) and `normalized_power_level` :387. `RoomPowerLevels::user_can_send_state`, `user_can_invite`, `user_can_kick` and friends (ruma-events room/power_levels.rs:631-770). Base `Room::power_levels()` :392.
- `Account` methods: `get_display_name` :118, `set_display_name(Option<&str>)` :139, `get_avatar_url` :194, and `upload_avatar(&Mime, Vec<u8>) -> OwnedMxcUri` :330, which also sets the avatar URL.

**Spaces (matrix-sdk-ui `src/spaces/`, `room_list_service/filters/`)**
- `SpaceService::new(client)` spaces/mod.rs:154, `space_filters()` :284 and `subscribe_to_space_filters()` :299. `SpaceFilter { space_room: SpaceRoom, level: u8, descendants: Vec<OwnedRoomId> }` :773 covers two levels.
- `SpaceRoom` (spaces/room.rs:33-75) has `room_id`, `display_name`, `avatar_url`, `children_count`, `state` and more.
- `new_filter_identifiers(Vec<OwnedRoomId>)` (filters/mod.rs:94) is the filter the SDK's own doc example pairs with a `SpaceFilter` (spaces/mod.rs:270-281). The core already composes filters in `room_list.rs:77-103`.

**Threads (matrix-sdk-ui `src/timeline/`, matrix-sdk `src/event_cache/`)**
- `TimelineFocus::{Live { hide_threaded_events }, Event { .. }, Thread { root_event_id }, PinnedEvents}` mod.rs:126.
- `TimelineBuilder`: `with_focus` builder.rs:74, `with_internal_id_prefix(String)` :92, and the default `Live { hide_threaded_events: false }` :63. `RoomExt::timeline_builder()` (traits.rs:80) adds read-receipt tracking; the core uses `room.timeline()` today (engine.rs:1327).
- `MsgLikeContent { thread_root: Option<OwnedEventId> :74, thread_summary: Option<ThreadSummary> :76 }`, `ThreadSummary { latest_event: TimelineDetails<Box<EmbeddedEvent>>, num_replies: u32 }` msg_like.rs:51-62. `TimelineItemContent::thread_summary()` content/mod.rs:513.
- Thread summaries are computed by the event cache from sync for every room (event_cache/caches/mod.rs:340-373) and from server-bundled thread data. That code does not check `ThreadingSupport`, which stays `Disabled` by default (client/builder/mod.rs:184, `with_threading_support` :615) and also changes unread counts and receipts.
- `hide_threaded_events` is read by the controller alone (controller/mod.rs:187) and does not need `ThreadingSupport`.
- `Timeline::send` on a thread-focused timeline sets the thread relation itself (mod.rs:500-510 `EnforceThread::Threaded`).

**iced 0.14 / winit 0.30.13 (UI)**
- Window events: `FileHovered(PathBuf)` :56, `FileDropped(PathBuf)` :66 and `FilesHoveredLeft` :76 (iced_core window/event.rs). The app already maps `Focused`/`Unfocused` to `Message::WindowFocus` (state.rs:1684).
- **winit 0.30.13 has no file drag and drop on Wayland.** `DroppedFile` is produced only by `platform_impl/windows/drop_handler.rs`, `macos/window_delegate.rs` and `linux/x11/event_processor.rs`; nothing under `platform_impl/linux/wayland`. Drag and drop works on X11, Windows and macOS and on XWayland windows, **not on a native Wayland window** (Q4, rule 8).
- **The iced clipboard is text-only:** `iced::clipboard::read/write` (iced_runtime clipboard.rs:29/:49) and window_clipboard 0.5.1 (`read`/`write` return `String`, lib.rs:66-80). Image paste needs `arboard` (Q5).
- `text_editor::key_binding` :313 with `Binding::Paste` :1127 and `Binding::Custom(Message)` :1149 lets the composer route Ctrl+V to the app (the composer already uses `key_binding`, composer.rs:131-151).
- Images: `image::Handle::from_bytes` :136 and `from_rgba` :147 (iced_core image.rs); `iced_widget` `image/viewer.rs` gives a zoom/pan viewer.
- Focus: `focus_next` :165, `focus_previous` :122 and `focus(Id)` :29 (iced_core widget/operation/focusable.rs).
- Window tasks: `window::run(id, FnOnce(&dyn Window) -> T)` :463 gives a raw window handle (for the dialog parent), plus `gain_focus` :431 and `request_user_attention` :415 (iced_runtime window.rs).
- **No AccessKit or screen-reader support** in any iced 0.14 crate (no `accesskit` in iced*, iced_winit, iced_widget).
- iced_winit 0.14.1 `run` (lib.rs:68-110) boots the program before `run_app`. On macOS a tray icon must be created only once the event loop runs (tray-icon lib.rs:21), so the tray is created on the first window-opened message (Q12).

**Candidate dependencies (rule 3; measured lock impact, Q3)**
- Probes: in the scratch copy, each candidate was added to `crates/app/Cargo.toml` and `cargo metadata` was run; then Cargo.lock name/version lists were compared before vs after. **No existing package version changes in any probe.**
  - `rfd = { version = "=0.17.2", default-features = false, features = ["xdg-portal", "wayland"] }`: +2 (rfd 0.17.2, pollster 0.4.0). On Linux it uses its own XDG portal code with a zenity fallback (`backend/xdg_desktop_portal.rs`, `backend/linux/zenity.rs`), no GTK. `AsyncFileDialog::pick_files` :273, `save_file` :328, `set_parent` :241, `set_file_name` :221. MIT.
  - `arboard = { version = "=3.6.1", default-features = false, features = ["image-data"] }`: +4 (arboard 3.6.1, tiff 0.11.3, fax 0.2.7, weezl 0.1.12). Linux backend is X11 (`x11rb`, already locked); the `wayland-data-control` feature (wl-clipboard-rs, wlr-data-control protocol) is not enabled. MIT OR Apache-2.0.
  - `notify-rust = "=4.18.2"` (default `z`: zbus 5 with async-io; zbus 5.19.0 with async-io is already locked through iced's `mundy`): +4 (notify-rust 4.18.2, mac-notification-sys 0.6.15, tauri-winrt-notification 0.8.1, windows-version 0.1.7). MIT OR Apache-2.0. **Published 2026-10-07** (4.18.1: 2026-09-27).
  - `tray-icon = { version = "=0.26.1", default-features = false, features = ["ksni"] }`: +7 (tray-icon 0.26.1, muda 0.21.1, ksni 0.3.6, crossbeam-channel 0.5.17, keyboard-types 0.8.3, pastey 0.2.3, task-local 0.1.1). The KSNI backend runs its D-Bus service on a worker thread with no GTK (lib.rs:17-18). MIT OR Apache-2.0, **except ksni 0.3.6: `Unlicense`, which `cargo deny check licenses` rejects** with today's `deny.toml`. **Published 2026-10-07** (0.26.0: 2026-09-30).
  - `tray-icon` with default features (libappindicator, GTK3 through `muda/gtk3`): +47 packages. Rejected (Q3).
  - `image` gains the `gif` feature: +2 (gif 0.14.2, color_quant 1.1.0; weezl shared with arboard).
  - **All of the above together: +19 packages, 0 removed or changed.** The complete list: arboard, color_quant, crossbeam-channel, fax, gif, keyboard-types 0.8.3, ksni, mac-notification-sys, muda, notify-rust, pastey, pollster, rfd, task-local, tauri-winrt-notification, tiff, tray-icon, weezl, windows-version. `cargo deny check`: advisories ok, bans ok (duplicate warnings only), sources ok, licences FAILED only on ksni's Unlicense.
- `crates/core` would gain `mime = "0.3"` as a direct dependency. It is 0.3.17, already in Cargo.lock, so no new package.

**Current repo state relevant here**
- `crates/app/src/state.rs` is about 6,000 lines. Phase 5 puts each feature in its own module and keeps `state.rs` a dispatcher, as Phase 4 did with `security.rs`/`verification.rs`/`sessions.rs`.
- Pills:
  - `Inline::Mention { target: MentionTarget::{User(UserId), Room(String)}, label }` (api/types.rs:386-418), where the label is sender-chosen;
  - the app's `Room` pill selects a listed room, an alias does nothing, and a `User` pill does nothing (PROGRESS, Phase 3 Task 10 notes).
- `RoomListTab::Spaces` exists (types.rs:134-144). `RoomSummary` has `is_space`, `is_invite`, `is_encrypted`, `dm_peer`.
- The core already fetches avatar thumbnails with a `Semaphore`, a 20 s deadline and refusal of `*.localhost` MXC servers (`crates/core/src/avatars.rs`). The app decodes them with `image` `Limits` (`crates/app/src/avatars.rs:20-25`, `decode_avatar` :137).
- Data root: the Phase 4 Q16 (c) test (`paths.rs`) asserts it holds only `last_account` and the store directory.
- Test accounts: `.env.test` has alice, bob and carol. Every E2EE test resets alice's and carol's crypto state under `e2ee_serial()` (Phase 4 Q2). The repeat-run recipe is `scripts/prune-test-rooms.sh`, then the ignored suite (Phase 4 wrap-up). The full ignored suite took about 7 to 7.5 minutes.

### Decisions (accepted by the human on 2026-10-08; see "Human answers recorded")
1. **Q1: precondition (rule 1).** ACCEPTED AS RECOMMENDED (2026-10-08). Phase 4 is signed off (2026-10-07) and merged (PR #7, 5d2f4fb). Task 1 records "Phase 4 signed off by the human on 2026-10-07 and merged (PR #7, 5d2f4fb); QR scan, OAuth and Windows/macOS items deferred or accepted as recorded in the Phase 4 checkpoint" and leaves the Phase 4 open issues as they are.
2. **Q2: platforms, CI and the checkpoint scope.** ACCEPTED (2026-10-08): the same rule as Phase 4's final decision.
   - **Gate:** the local gate only (fmt, clippy, tests, boundary scripts, `cargo deny` on Linux). Phase 5 is not gated on a green CI run.
   - **Checkpoint:** human checks on Linux, Wayland and X11.
   - **Unverified on Windows and macOS:** these code paths (WinRT and macOS notifications, Win32/AppKit tray, native dialogs, arboard on Windows/macOS) are **not even compiled** locally (cross-compiling the SDK's aws-lc needs a cross C toolchain). The risk is accepted explicitly, and the code stays behind `cfg` only where the crates need it.
   - **Optional, not required:** pushing the branch would let CI's Windows and macOS jobs at least build it (the earlier CI run failed for billing before any job ran). No task and no checkpoint item depends on it.
3. **Q3: new dependencies (rule 3).** ACCEPTED (2026-10-08), with the latest versions: add exactly these, in one task (Task 2), with the measured +19 lock packages and nothing else changed.

   | Crate | Proposed pin | Features | Why | Lock |
   | --- | --- | --- | --- | --- |
   | `rfd` | `=0.17.2` | `default-features = false`, `xdg-portal`, `wayland` | native open/save dialogs (roadmap); portal on Linux, no GTK | +2 |
   | `arboard` | `=3.6.1` | `default-features = false`, `image-data` | read images from the clipboard (iced reads text only) | +4 |
   | `notify-rust` | `=4.18.2` | default (`z`) | desktop notifications (roadmap) | +4 |
   | `tray-icon` | `=0.26.1` | `default-features = false`, `ksni` | tray icon with badge (roadmap), no GTK on Linux | +7 |
   | `image` (existing, `0.25` → 0.25.10 unchanged) | unchanged | add `gif` | show GIFs (first frame) | +2 |
   | `mime` (core, new direct dep) | `"0.3"` (0.3.17, already locked) | default | name the `Mime` that `send_attachment` takes | 0 |

   - **`deny.toml` change needed (accepted):** a `[[licenses.exceptions]]` entry `{ allow = ["Unlicense"], crate = "ksni" }`. Unlicense is a public-domain dedication, compatible with GPL-3.0-or-later. Same style as the existing `webpki-root-certs` exception.
   - **Very recent releases:** notify-rust 4.18.2 and tray-icon 0.26.1 were published on 2026-10-07. The previous patch releases are 4.18.1 (2026-09-27) and 0.26.0 (2026-09-30). The human chose the latest patches: `notify-rust =4.18.2` and `tray-icon =0.26.1` (no re-measure needed).
   - **Exact `=` pins** for the four OS-integration crates: they touch OS APIs whose behaviour can change in patch releases. `image` keeps its caret, as today.
   - Rejected:
     - tray-icon default features (+47 packages, GTK3 bindings through `libappindicator`/`muda/gtk3`);
     - rfd `gtk3`;
     - arboard `wayland-data-control` (wlr-data-control, which GNOME does not implement);
     - `mime_guess`/`infer` (a small in-house extension table plus `image::guess_format` covers what Tessera sends);
     - `blurhash`;
     - animated GIF playback (optional in the roadmap; the first frame only);
     - inline video/audio playback (roadmap: "play video externally at first").
4. **Q4: drag and drop on Wayland (rule 8: a roadmap item the pinned stack can't do).** ACCEPTED (2026-10-08): accept the gap.
   - Drag and drop is wired through iced's `FileHovered`/`FileDropped`/`FilesHoveredLeft`. It works on X11 (and XWayland), Windows and macOS. On a native Wayland window winit 0.30.13 never emits the events, so nothing happens.
   - The file picker and clipboard paste are the Wayland paths. The checkpoint tests drag and drop on X11, and records the Wayland gap in PROGRESS.md and in the user-facing shortcuts list ("drag and drop is not available on Wayland yet").
   - Rejected: forcing XWayland for the whole app (loses native Wayland scaling and input; the Phase 6 portal hotkeys need Wayland); a winit/iced upgrade (pinned).
5. **Q5: clipboard image paste.** ACCEPTED (2026-10-08):
   - The composer binds Ctrl+V (Cmd+V on macOS) to `Binding::Custom(PasteRequested)`.
   - The app then reads the clipboard **off the UI thread** in a `Task`: `arboard::Clipboard::get_image()` first.
     - An image is encoded to PNG (`image` crate) and goes to the upload confirmation (Q9).
     - Otherwise it reads text with `get_text()` and inserts it with `text_editor::Action::Edit(Edit::Paste(..))`, so text paste keeps working.
     - If arboard fails (no X server, for example), the app falls back to `iced::clipboard::read()` for text.
   - On Wayland, arboard (no data-control) reads the X11 clipboard through XWayland, which GNOME and KDE keep in sync. This is a checkpoint item.
   - Rejected: `wayland-data-control` (not on GNOME); a separate "paste image" shortcut (unexpected for users).
6. **Q6: media API shape (additive, Phase 2 Q6 convention).** ACCEPTED AS RECOMMENDED (2026-10-08).
   - **No SDK type, MXC URI or encryption key crosses the API.** The JWK key, IV and hashes of an encrypted file are secrets. Media is addressed by `(room_id, item_id)`, and the core looks the item up in the room's live or thread timeline, as `Edit`/`Redact` do.
   - **Received media:**
     - `TimelineItemKind::Message` items gain `details.media: Option<Box<MediaContent>>`;
     - `MediaContent { kind: MediaKind { Image, Video, Audio, File }, filename, mimetype: Option<String>, size: Option<u64>, width: Option<u32>, height: Option<u32>, duration_ms: Option<u64>, has_thumbnail, upload: Option<UploadProgress { sent: u64, total: u64 }> }`;
     - the message `body` is the caption, or the file name when there is none;
     - stickers stay placeholders;
     - reply, react, redact and shields then work unchanged. Edit is offered only for the caption of the user's own media and is off when there is no caption. `item_actions` gains a `media` input.
   - **Commands:**
     - `FetchMedia { room_id, item_id, size: MediaSize { Thumbnail, Full } }`, answered by `CoreEvent::Media { room_id, item_id, size, result: Result<MediaBytes, CoreError> }`;
     - `SaveMedia { room_id, item_id, path: PathBuf }`, answered by `MediaSaved { room_id, item_id }` or `ActionFailed { Media, .. }`;
     - `SendAttachment(OutgoingAttachment)` (Q9).
   - **`MediaBytes`** is an `Arc<[u8]>` newtype whose `Debug` prints the length only. `MediaContent`'s `Debug` hides the file name (user content).
   - **Limits:** `CoreEvent::MediaLimits { max_upload_bytes: Option<u64> }` is sent after login or restore.
   - **`ActionKind` gains** `Media`, `Upload`, `Notifications`, `Rooms`, `Profile`, `Thread`.
   - Rejected: a `Media` variant of `TimelineItemKind` (it would duplicate the message plumbing for reply, react and shields); MXC strings in the API (they bypass the item check and invite fetching arbitrary media).
7. **Q7: fetch, cache and memory.** ACCEPTED AS RECOMMENDED (2026-10-08):
   - **Thumbnails:**
     - plain media: `get_thumbnail` at 800x600 `Scale` (server-side; Synapse makes it);
     - encrypted media: the `thumbnail_source` when there is one, else the full file **only if** `info.size <= 10 MiB`, else a "no preview" card;
     - at most 4 fetches at once (the avatar semaphore pattern), each with a 30 s deadline;
     - `use_cache = true`, so the SDK media store caches the content, encrypted at rest.
   - **Full files:** fetched only on demand (viewer, Save), with a 10-minute deadline. They are refused before the request when `info.size` exceeds 512 MiB, because the SDK holds the whole file in memory. The residual: a lying `info.size` can still make the SDK allocate the real size.
   - **App:**
     - images are decoded off the UI thread, with `image` `Limits` (16384 px per side, 256 MiB allocation);
     - thumbnails are scaled down to at most 400x300 for the timeline before upload to the renderer;
     - a byte-budgeted LRU of decoded `Handle`s: 64 MiB for thumbnails, 1 full image at a time;
     - fetches are requested only for rows inside the Phase 3 visible window plus one screen of margin.
   - **Row height:** the media box size comes from `info.width/height`, scaled to fit 400x300, or 320x180 when unknown. It is fixed before the bytes arrive, so a loading image never changes the row height (Phase 3 measured heights, no scroll jumps).
   - **GIF:** first frame only (Q3).
8. **Q8: files, video and audio; saving and opening.** ACCEPTED AS RECOMMENDED (2026-10-08):
   - **Cards:** video, audio and file messages show a card with an icon, the file name (sanitised by `text_safety`: bidi controls removed, so `gnp.exe` tricks show as typed; capped), the human size and the type.
   - **Save…** opens `rfd::AsyncFileDialog::save_file()`:
     - parented to the window via `window::run`;
     - the suggested name is the sanitised file name with path separators removed;
     - then `SaveMedia` runs: the core fetches, writes `<path>.tessera-part` in the same directory, `fsync`s, and renames.
   - **Open:** a card offers **Open** only after a successful save. It runs `open::that(saved_path)` in a `Task`, behind the Phase 3 confirmation bar ("Open <file name>?").
   - **Dangerous extensions** are never opened: `exe, msi, bat, cmd, com, scr, ps1, vbs, js, jar, sh, desktop, appimage, app, pkg, dmg, deb, rpm, run`, and anything without an extension. They can only be saved.
     *Changed by the human at the checkpoint (2026-10-08):* any saved file may be opened after the confirmation; for these extensions (extended after the Task 9 audit) the bar adds a warning that the file can run a program.
   - **Video:** "play externally" is Save → Open.
   - **No decrypted temp files:** Tessera never writes decrypted media to a temp directory (`get_media_file` is not used).
   - Rejected: opening from a hidden temp file (decrypted content left on disk in clear, outside the user's view); inline playback.
9. **Q9: upload pipeline.** ACCEPTED AS RECOMMENDED (2026-10-08):
   - **App side (`crates/app/src/upload.rs`, pure where possible):**
     - **Sources:** the picker (`pick_files`), drop events, and a pasted image (an in-memory PNG named `pasted-image-<yyyymmdd-hhmmss>.png`).
     - **Type:** the MIME type comes from a small extension table, cross-checked with `image::guess_format` for images. A file whose bytes are not the image it claims to be is sent as a file. Unknown types are `application/octet-stream`.
     - **Thumbnails, for images only:**
       - the source is decoded with `Limits`, and EXIF orientation is applied (`ImageDecoder::orientation`, `DynamicImage::apply_orientation`);
       - it is scaled to fit 800x600 (never upscaled);
       - encoded as JPEG quality 80, or PNG when the image has alpha;
       - with the original's width and height. A thumbnail is made only when the original is larger than 800x600 or bigger than 1 MiB.
       - No thumbnail for video (no decoder) or other files.
     - **Original bytes are sent unchanged:** EXIF metadata, including GPS, stays in the original, as in Element Web. This is a privacy residual written into the confirmation dialog ("Photos are sent as they are, including location data the camera saved"). Stripping metadata would re-encode the photo and lose quality.
     - **Before sending:** files larger than `MediaLimits.max_upload_bytes` (or 100 MiB when unknown) are refused in the dialog.
     - **Confirmation dialog:** it lists every file (name, size, preview for images) with **Send** and **Cancel**. Multiple files become one message each, in order. There is no caption entry in Phase 5; received captions are shown.
   - **Core side:**
     - `SendAttachment(OutgoingAttachment { room_id, filename, mimetype, data: AttachmentData { Path(PathBuf), Bytes(MediaBytes) }, kind: MediaKind, width, height, thumbnail: Option<UploadThumbnail { data, mimetype, width, height }>, reply_to: Option<EventId>, thread_root: Option<EventId> })`;
     - the core reads a `Path` with `tokio::fs::read`, not the SDK's blocking `fs::read`, then calls `Timeline::send_attachment(AttachmentSource::Data { .. }, mime, config).use_send_queue()`;
     - `client.send_queue().enable_upload_progress(true)` is set once after login and restore, so local echoes carry progress;
     - `DiscardSend` cancels an upload that has not started; once it has started, the SDK may refuse, and the app shows "can't cancel now".
   - Rejected: generating thumbnails in the core (would put an image codec into the core; the app already decodes images); sending without the send queue (no local echo for media); EXIF stripping by default (quality loss; Element Web doesn't).
10. **Q10: notifications.** ACCEPTED AS RECOMMENDED (2026-10-08):
    - **Core (`crates/core/src/notifications.rs`):**
      - registers one `register_notification_handler` per client;
      - maps each `Notification` through a pure, table-tested `classify(..)` that drops:
        - the user's own events;
        - events older than `session_start - 60 s` (initial sync and limited backfills);
        - duplicate event IDs (a 512-entry ring);
        - non-message events other than invites for the user;
      - emits `CoreEvent::Notification(NotificationView { room_id, event_id, room_name, sender: UserId, sender_name, is_dm, kind: NotificationKind { Message, Mention, Invite, Encrypted }, preview: Option<MessageText> })`;
      - `Mention` is set when the actions carry `SetTweak(Highlight)`;
      - `Encrypted` covers events still encrypted when evaluated. The preview says "Encrypted message", and the SDK's late decryption does **not** re-notify (residual);
      - the preview is the plain body's first 120 characters (no formatting, spoilers replaced by "(spoiler)");
      - `NotificationView`'s `Debug` hides the preview and names.
    - **App (`crates/app/src/notifications.rs`):** a pure `should_show(view, window_focused, selected_room, settings) -> bool`:
      - shows only while the window is **unfocused** (roadmap);
      - never for the selected room while it is focused;
      - only when desktop notifications are on;
      - at most one per room every 3 s (later ones in that window are folded into a "N new messages" update of the same notification, using notify-rust's `id` replacement on Linux).
    - **What a notification shows:**
      - the body is the sanitised sender name and preview;
      - with "Show message content" off, the body is "New message" (or "Mentioned you", or "Invited you");
      - the title is the room name; DMs use the sender's name.
    - **Sending** goes through a `Notifier` trait, so tests use a fake. The real one, `notify-rust`, runs in a `Task` (a blocking D-Bus call, off the UI thread).
    - **Clicking (Linux only):** the "default" action of a notification opens the room and calls `window::gain_focus`. On Windows and macOS clicks only raise the OS default behaviour (not tested, Q2).
    - **Taskbar/dock:** `request_user_attention(Informational)` for mentions.
    - Rejected: polling unread counts (no per-event content, no mention distinction); a separate `NotificationClient` (needed only for push without sync); custom sounds.
11. **Q11: notification settings.** ACCEPTED AS RECOMMENDED (2026-10-08):
    - **Room header menu:** "All messages", "Mentions and keywords", "Mute" (`SetRoomNotificationMode { room_id, mode }`), plus "Use default" (`delete_user_defined_room_rules`).
    - **Room list:** `RoomSummary.notification_mode: Option<RoomNotificationModeView>` comes from `cached_user_defined_notification_mode()`. A muted room shows a muted bell, and its unread badge turns grey (no count highlight unless mentioned).
    - **Settings → Notifications:**
      - default modes for group rooms and for DMs: `SetDefaultNotificationMode { one_to_one: bool, mode }`, applied to encrypted and unencrypted alike, as Element X does;
      - keywords: `AddKeyword` and `RemoveKeyword`, each trimmed, 1-64 characters, at most 50;
      - the local toggles: desktop notifications on/off, show message content (default **on**, as Element), tray badge on/off.
    - **Updates:** `CoreEvent::NotificationSettings(NotificationSettingsView { group_default, dm_default, keywords })` is sent after login and on each `subscribe_to_changes` tick.
    - Rejected: editing raw push rules; per-room keyword rules.
12. **Q12: tray icon.** ACCEPTED AS RECOMMENDED (2026-10-08):
    - **Backend:** tray-icon with the KSNI backend on Linux; the native backends on Windows and macOS.
    - **Creation:** in `update` on the first window-opened message (main thread, event loop running; macOS rule). Failures are logged without detail and the app runs without a tray. GNOME needs the AppIndicator extension; this is a checkpoint item.
    - **Icon:** drawn in code as RGBA (no asset file), 32x32 and 64x64, in two variants: plain, and with a red dot when `UnreadTotals.notifications > 0`.
    - **Tooltip:** "Tessera" or "Tessera – N unread", with N capped at "99+".
    - **Menu:** **Show Tessera** (`gain_focus`, un-minimise) and **Quit** (the existing close path, so the Phase 4 clipboard flush still runs).
    - **Events:** tray and menu events cross from the `crossbeam` receivers into iced through a `Subscription` on a small forwarding thread.
    - **Totals:** `CoreEvent::UnreadTotals { notifications, highlights }` is computed by the core over **all** joined, non-muted rooms. The app may hold only the first room-list page, so it can't sum them itself. It is emitted at most once per second, on change.
    - **Closing the window still quits.** iced 0.14 has no hide-window task (`window.rs` function list), so "close to tray" is out of scope.
    - Rejected: libappindicator/GTK (Q3); a number drawn in the icon (needs a font rasteriser; the tooltip carries the count).
13. **Q13: rooms.** ACCEPTED AS RECOMMENDED (2026-10-08) (core `crates/core/src/rooms.rs`, app `crates/app/src/rooms.rs` + `screens/room_dialogs.rs`):
    - **Create room:**
      - fields: name (required, 1-255), topic (optional), **Encrypted** (on by default, can't be turned off later; closes the Phase 4 Q17 deferral), visibility Private (invite-only, `PrivateChat` preset) or Public (`PublicChat` preset plus a required alias local part), and invitees;
      - encryption goes in `initial_state` as `m.room.encryption` with `m.megolm.v1.aes-sha2`;
      - `CreateRoom` gives `RoomCreated { room_id }`, and the app selects it once it appears in the list.
    - **Start a DM:** `CreateDm { user_id }` reuses an existing DM with that user if `m.direct` names one, else calls `create_dm`. It gives `RoomCreated`.
    - **Join:** "Join a room" takes `#alias:server`, `!id:server` or a `matrix.to` link. The pure `parse_room_target` is shared with pills. `JoinRoom { target, via: Vec<String> }` gives `RoomJoined { room_id }`.
    - **Invites** show in the list (`is_invite`) with **Accept** (`AcceptInvite`, `Room::join`) and **Decline** (`DeclineInvite`, `Room::leave`).
    - **Invite:** `InviteUser { room_id, user_id }` from the member list. The user ID is validated in the core.
    - **Leave:** behind a confirmation. In an encrypted room, or one that is the user's last admin seat, the confirmation warns. `LeaveRoom { room_id }` deselects the room.
    - **Room settings panel:** name, topic, avatar (picked with rfd, encoded as for uploads, at most 1 MiB after a 512x512 downscale), and "Enable encryption" (one-way, with a warning).
      - Each control is enabled only if the power-level check passes. `RoomDetails { room_id, name, topic, avatar, is_encrypted, alias, permissions: RoomPermissions { can_set_name, can_set_topic, can_set_avatar, can_enable_encryption, can_invite } }` comes from `OpenRoomDetails { room_id }` and refreshes on state changes while open.
    - **Member list:** `ListRoomMembers { room_id }` gives `RoomMembers { room_id, members: Vec<RoomMemberView { user_id, display_name, avatar, power: PowerLevelView { Creator, Level(i64) }, role: Role { Creator, Admin, Moderator, User }, membership: Joined|Invited }>, truncated: bool }`.
      - The list is capped at 5,000 and sorted by role, then name.
      - Display only, plus Invite.
    - **Own profile:** `SetDisplayName { name }`, `SetAvatar { data: MediaBytes, mimetype }`, `RemoveAvatar`, and `CoreEvent::OwnProfile { display_name, avatar: Option<AvatarId> }` sent after login and on change.
    - Rejected for Phase 5: editing power levels, kick and ban (the roadmap asks for the list "with power levels"; changing them is a moderation feature with its own risks); room upgrades; room directory browsing; knocking.
14. **Q14: pills (Phase 3 deferrals).** ACCEPTED AS RECOMMENDED (2026-10-08):
    - **User pill label:** built from the **target**, never from the sender's label.
      - The text is the target's display name when the app knows the member (from `RoomMembers` of the open room, or the sender profiles already in the timeline), else the full user ID (elided in the middle beyond 48 characters).
      - A tooltip always shows the user ID.
    - **Room pill label:** the room's name when it is in the room list, else the alias or ID.
    - **Clicking a user pill** opens a small user card: avatar, name and the full sanitised ID on its own line, with **Message** (`CreateDm`) and, for the own user, nothing.
    - **Clicking a room pill:** a joined room is selected; otherwise a bar asks "Join <alias or ID>?" with **Join** (`JoinRoom`) and **Cancel**.
    - The pill label rule lives in a pure, table-tested `pill_label(target, members, rooms)`.
    - Rejected: showing the sender's label (the Phase 3 spoofing risk); a full profile screen.
15. **Q15: spaces.** ACCEPTED AS RECOMMENDED (2026-10-08):
    - **Core:**
      - the core owns one `SpaceService` per session and forwards `subscribe_to_space_filters` as `CoreEvent::Spaces(Vec<SpaceView { id, name, avatar, level, children: u64 }>)`;
      - `RoomListQuery` gains `space: Option<RoomId>`. When set, the core adds `new_filter_identifiers(descendants)` to the filter set and keeps the tab and search. An unknown space ID shows an empty list.
    - **App:** a narrow sidebar at the left of the room list: **Home** (no filter) and one avatar or initials button per top-level space (level 0), with tooltips. Level-1 spaces show indented under the selected one. The selected space is kept per session.
    - The existing **Spaces** tab stays (it lists space rooms).
    - Rejected: creating or editing spaces (the roadmap asks for a filter only); space hierarchy browsing.
16. **Q16: threads.** ACCEPTED (2026-10-08):
    - **Main timeline:** built with `TimelineFocus::Live { hide_threaded_events: true }` through `room.timeline_builder()`, so the read-receipt tracking stays. Threaded replies leave the main timeline, as in Element.
    - **`ThreadingSupport` stays `Disabled`:** no change to unread counts or receipt semantics. If thread summaries do not appear without it (checked in Task 8), the task stops and asks (rule 8).
    - **Summaries:** `MessageDetails.thread: Option<ThreadSummaryView { replies: u32, latest_sender: Option<UserId>, latest_sender_name, latest_preview: Option<String> }>`. The row shows "N replies · <name>: <preview>".
    - **Opening a thread:**
      - `OpenThread { room_id, root: EventId }` builds a second `Timeline` with `TimelineFocus::Thread { root_event_id }` and `with_internal_id_prefix("thread:")`;
      - it emits `ThreadDiff { room_id, root, diffs }` and `ThreadPagination { .. }`;
      - one thread is open at a time; `CloseThread` stops it, and so do selecting another room and logout.
    - **Item IDs that start with `thread:`** route `Edit`, `Redact`, `ToggleReaction`, `RetrySend`, `DiscardSend`, `FetchMedia` and `SaveMedia` to the thread timeline (pure `route_item(id)`).
    - **Sending:** `OutgoingMessage` gains `thread_root: Option<EventId>`. When set, the core sends through the thread timeline, which adds the relation (mod.rs:500-510). `OutgoingAttachment` has the same field.
    - **App:** a right-hand side panel that reuses the Phase 3 timeline pane and composer components (its own scroll state, its own draft kept in memory only). **Esc** or **×** closes it.
    - Rejected: a thread list (`ThreadListService` exists but isn't asked for); thread subscriptions (MSC4306); threaded read receipts; per-thread drafts on disk.
17. **Q17: Settings screen and local settings.** ACCEPTED AS RECOMMENDED (2026-10-08): one Settings screen, opened from the account area, with these sections:
    - **Profile:** display name field with Save; avatar with Change and Remove;
    - **Notifications:** Q11;
    - **Appearance:** theme System, Light or Dark (today it follows the system only);
    - **Keyboard shortcuts:** a read-only list (Q18), including the Wayland drag-and-drop note;
    - **Push-to-talk:** a placeholder line: "Push-to-talk arrives with voice calls in a later version.";
    - **Sessions:** a link to the existing screen.

    Local settings (theme override, desktop notifications, show content, tray badge) are stored as a small JSON file `<data root>/settings.json`, written atomically through new `DataPaths::read_settings`/`write_settings`. The file holds the typed `api::UiSettings` (no secrets, no content), so `paths` stays SDK-free and the app needs no serde dependency. The Phase 4 Q16 (c) data-root test is updated to allow exactly `settings.json` besides `last_account` and the store directory.
    - Rejected: syncing settings as account data (other clients would see Tessera-only keys); custom shortcut rebinding (Phase 6 does the PTT binding).
18. **Q18: accessibility pass.** ACCEPTED AS RECOMMENDED (2026-10-08):
    - **Ctrl+K** (Cmd+K on macOS) opens a room switcher: a text field plus a list. The fuzzy match is a pure, tested function over names and aliases the app holds, falling back to the core's room-list search for rooms not loaded yet. ↑/↓ move, Enter opens, Esc closes.
    - **Room list:** with the list focused, Alt+↑/Alt+↓ move to the previous or next room from anywhere, and ↑/↓ move while it is focused. Ctrl+Shift+↑/↓ go to the next unread room.
    - **Tab and Shift+Tab** move focus in a fixed order (sidebar → room list → timeline → composer) using `focus_next`/`focus_previous`.
    - **Esc** closes the top overlay. **Ctrl+,** opens Settings, and **Ctrl+U** attaches a file.
    - Every shortcut is in the Settings list.
    - **The screen-reader gap is documented** in `docs/accessibility.md`: iced 0.14 has no AccessKit integration (verified: no `accesskit` in any iced 0.14 crate). The file covers what works, the upstream tracking issue to watch, and a re-check at each iced upgrade.
    - **IME:** the deferred Phase 3 IME check goes on this checkpoint.
19. **Q19: test accounts, runtime and isolation (rule 6).** ACCEPTED AS RECOMMENDED (2026-10-08):
    - **Accounts:** non-E2EE tests (rooms, spaces, threads, notifications, plain media) use alice and carol, as the timeline tests do, and leave bob alone. Encrypted-media tests take `e2ee_serial()` and `reset_e2ee_account` for alice and carol, as Phase 4 Task 3 did.
    - **Push rules are account data** and survive between runs. `tests/common` gains `reset_notification_baseline(n)`, which the notification tests call first. It removes every keyword starting with `tessera-kw-`, sets both default modes back to the server defaults, and deletes user-defined rules for rooms the test created. Keywords are unique per run (`tessera-kw-<hex>`).
    - **Negative tests** ("no notification") are bounded: after the event that must not notify, the test sends a second, notifying sentinel in another room. It asserts the sentinel arrives first and nothing for the muted event arrived in between, with a 30 s overall limit (no fixed sleeps).
    - **Room growth:** every task's acceptance runs `scripts/prune-test-rooms.sh` before an ignored run. Room and space tests leave the rooms they create at the end.
    - **Time:** each task records the ignored-run duration. If the full suite passes 25 minutes, stop and ask (rule 8). (Raised from 15 minutes by the human on 2026-10-11: the suite ran 15 min 6 s after pruning.) A test failing twice in a row is a rule-8 stop.
    - Rejected: new test accounts (Phase 4 Q2 answer); mock-server tests for these flows.
20. **Q20: content and secret hygiene (rule 6).** ACCEPTED AS RECOMMENDED (2026-10-08):
    - **Never logged:** media bytes, file names, captions, notification previews and keywords. Every new public type that holds them has a redacted `Debug`, tested with sentinels (`secret-body-x`, `secret-name-x.png`).
    - **Never across the API or into logs:** encryption keys of media (`EncryptedFile`).
    - **Text safety:** file names, room names, topics, member names and space names from the server pass through `text_safety` before display. File names suggested to the save dialog are reduced to a single path component.
    - **Data at rest:** `log_hygiene` (Phase 4 Task 9) gains a sent and fetched encrypted image whose bytes contain a sentinel. The data-at-rest scan must not find the sentinel in clear under the data root (the media store encrypts values).
    - **Residuals:**
      - EXIF in sent originals (Q9);
      - files the user saves are in clear by the user's choice;
      - notification text is handed to the OS notification service, where other apps or history may keep it ("Show message content" off avoids it);
      - the clipboard image read is not zeroized;
      - a lying `info.size` (Q7);
      - no re-notify after late decryption (Q10).
21. **Q21: scope cuts.** ACCEPTED AS RECOMMENDED (2026-10-08), out of Phase 5:
    - animated GIF and inline video/audio playback;
    - captions on send; image editing; galleries (MSC4274); stickers (still placeholders); voice messages; location sending;
    - link previews; blurhash;
    - "close to tray";
    - editing power levels, kick and ban; room upgrades; room directory; knocking;
    - creating or editing spaces;
    - thread list, thread subscriptions, threaded read receipts;
    - shortcut rebinding;
    - push notifications without a running client (pushers); notification sounds;
    - i18n;
    - and the Windows/macOS verification per Q2.

### Conventions for every task
- **Gate (rule 4)**, required before each commit:
  - `cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && cargo test --workspace --locked`, plus `scripts/check-app-deps.sh`, `scripts/check-core-api.sh` and `cargo deny check`.
  - Tasks that touch the core or the bridge also run `docker compose up -d --wait && scripts/prune-test-rooms.sh && cargo test -p tessera-core -p tessera-app --locked -- --ignored --skip keyring`, then `docker compose stop`, and record the duration.
  - Core tasks that add encrypted-media tests run that test binary a second time right after, with no reset in between (Phase 4 Q2 repeat runs).
  - The agent never runs the real-keychain test.
- **One commit per task**, message `phase5.<task number>: <summary>`, with the attribution trailer as instructed by the session. ROADMAP.md is never committed.
- **PROGRESS.md (rule 5).**
  - Task 1 adds "Phase 5 steps" (one line per task, with the roadmap steps it covers) and the "Phase 5 human checkpoint" list.
  - Each task ticks its line and adds a "Phase 5 / Task N notes" block with:
    - the checks run and their results, and the ignored-run duration;
    - every matrix-sdk(-ui/-base), ruma, iced, winit, rfd, arboard, notify-rust or tray-icon API used, with the registry file:line where it was confirmed;
    - open issues and new human items, which also go on the checkpoint list (and the checklist artifact) unasked.
  - Rendering, OS dialogs, clipboard, notifications, tray, drag and drop, keyboard-only use and IME go on the human list, never ticked by the agent (rule 9).
- **Boundary (rule 7).**
  - `crates/app` gains only the Q3 dependencies and imports only `tessera_core::{api, Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE, paths, secrets, logging}`.
  - `api/` stays SDK-free. New core modules (`media.rs`, `upload.rs`, `notifications.rs`, `rooms.rs`, `spaces.rs`, `threads.rs`) are private.
- **No network, file I/O, decoding or blocking in `update`.** `update` only mutates state, calls `try_send`, and returns `Task`s:
  - decode, thumbnail and clipboard work runs in `Task::perform` on blocking-safe workers;
  - dialogs, notifications and `open` run in `Task`s too.
  - Unit tests drive `update` with a plain `mpsc` sink and fake `Notifier`/`Clipboard`/`Dialogs` traits.
- **Fixed widget-tree shape:** new overlays (viewer, dialogs, switcher, user card, settings) and the thread panel use the existing `stack` layer pattern, so opening them doesn't reset the timeline scroll or the composer focus (Phase 3 rule; a tag test per overlay).
- **Stop and ask (rule 8)** if:
  - an API above behaves differently from this plan (for example thread summaries need `ThreadingSupport`, the send queue refuses encrypted attachments, the notification handler never fires under sliding sync, or Synapse refuses `initial_state` encryption);
  - a test is flaky twice in a row;
  - the ignored suite passes 25 minutes (raised from 15 on 2026-10-11);
  - `cargo deny` flags something beyond the ksni licence;
  - a step needs a real account or a non-Linux machine.

### Task 1: Phase 5 ledger, sign-off record and test helpers
Language: rust
Files: PROGRESS.md, crates/core/tests/common/mod.rs, crates/core/tests/fixtures/, crates/core/tests/rooms.rs
Depends on: none (Q1: Phase 4 signed off and merged)
Acceptance criteria:
- PROGRESS.md contains the Q1 sign-off line and the human's Phase 5 answers (once validated). It gains "Phase 5 steps" (Tasks 1-17 with roadmap steps) and a "Phase 5 human checkpoint" list seeded with the items at the end of this plan. "Pinned versions" gains a Phase 5 line only in Task 2.
- `tests/common/mod.rs` gains:
  - `create_room_raw(n, name, encrypted, invite)`, `create_space_raw(n, name, children)` (`creation_content.type = m.space` plus `m.space.child` state), `leave_all_created(n, ids)`;
  - `reset_notification_baseline(n)` (Q19);
  - `send_attachment_raw(n, room, filename, mime, bytes)` (raw SDK client `Room::send_attachment`, for download tests before the core can upload);
  - `fetch_raw_event_json(client, room, event)`.

  Each calls `assert_local_homeserver` first and prints only account names and counts.
- `tests/fixtures/` gains a 1,200x900 PNG with a `tEXt` sentinel chunk and a 64x48 PNG (committed fixture files, under 200 KiB in total), plus a JPEG with an EXIF orientation tag of 6 for the Task 10 thumbnail test.
- `crates/core/tests/rooms.rs` holds ignored smoke tests (verbatim ignore reason):
  - `create_room_raw` with `encrypted = true` gives a room whose `m.room.encryption` state carol can read;
  - `create_space_raw` gives a room with `room_type == m.space` and one `m.space.child`;
  - `reset_notification_baseline` leaves no `tessera-kw-` keyword (read back through `NotificationSettings::enabled_keywords`).
- `.env.test`, `scripts/create-test-users.sh` and bob's tests are unchanged.
- Gate and the ignored run pass. Commit: `phase5.1: phase 5 ledger, sign-off and test helpers`.

### Task 2: Phase 5 dependencies and licence triage
Language: rust
Files: crates/app/Cargo.toml, crates/core/Cargo.toml, Cargo.lock, deny.toml, PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- `crates/app/Cargo.toml` gains exactly the Q3 lines as validated (default: `rfd =0.17.2` with no default features plus `xdg-portal`, `wayland`; `arboard =3.6.1` with no default features plus `image-data`; `notify-rust =4.18.2`; `tray-icon =0.26.1` with no default features plus `ksni`; `image` gains `gif`). `crates/core/Cargo.toml` gains `mime = "0.3"`. No other manifest line changes; iced and matrix-sdk pins are unchanged.
- A sorted name/version comparison of Cargo.lock before vs after lists exactly the 19 Q3 packages as added, none removed or changed (or the re-measured list if the human picks other versions; then stop and ask if it differs from the measurement).
- `deny.toml` gains only the scoped `ksni` Unlicense exception. `cargo deny check` passes.
- `scripts/check-app-deps.sh` still exits 0 (none of the new crates match the SDK pattern).
- `cargo tree -p tessera-app -e normal --depth 1` lists the four crates; `cargo tree -p tessera-app -i gtk-sys --target all` finds nothing.
- PROGRESS.md "Pinned versions" gains the Phase 5 line with the lock diff.
- Gate passes (no core code touched; no ignored run). Commit: `phase5.2: phase 5 dependencies and ksni licence exception`.

### Task 3: Additive core API for Phase 5
Language: rust
Files: crates/core/src/api/media.rs, crates/core/src/api/rooms.rs, crates/core/src/api/mod.rs, crates/core/src/api/types.rs, crates/core/src/engine.rs, crates/core/src/timeline.rs, crates/core/src/room_list.rs, crates/core/src/bin/tessera-cli.rs, crates/app/src/state.rs, crates/app/src/timeline.rs, PROGRESS.md
Depends on: Task 2
Acceptance criteria:
- The API has exactly the Q6, Q9-Q17 additions: types, commands, events, `ActionKind` variants, `MessageDetails.media`/`thread`, `RoomSummary.notification_mode`, `RoomListQuery.space`, `OutgoingMessage.thread_root`, `UiSettings`.
  - All are `Clone + Debug + PartialEq` (`MediaBytes` compares by content), SDK-free and documented. `scripts/check-core-api.sh` exits 0.
  - Existing constructors give the new fields `None`/`false`/defaults, and every existing test passes unchanged in meaning.
- Redaction unit tests with sentinels: no sentinel appears in `format!("{:?}")` of `MediaBytes`, `MediaContent` (file name), `OutgoingAttachment`, `NotificationView` (preview and names), `Command::AddKeyword`, `Command::SetDisplayName`, `Command::SetAvatar`, `CoreEvent::Media` or `UiSettings`.
- Before the real handlers land, the engine answers every new command without panicking: `NotLoggedIn` with no session, `ActionFailed { action, error: Internal("not implemented yet") }` with a session. Unit-tested with the engine test helper (no network).
- The CLI matches the new events exhaustively and prints one non-secret line each (never previews, file names or bytes). Its tests pass.
- Gate and the ignored run pass. Commit: `phase5.3: additive core api for media, notifications, rooms, spaces and threads`.

### Task 4: Media in the timeline: mapping, fetch and save in the core (roadmap steps 1 and 2, core side)
Language: rust
Files: crates/core/src/media.rs, crates/core/src/timeline.rs, crates/core/src/engine.rs, crates/core/src/lib.rs, crates/core/tests/media.rs, crates/core/tests/log_hygiene.rs, PROGRESS.md
Depends on: Task 3
Acceptance criteria:
- `map_event` maps image, video, audio and file messages to `Message` with `details.media` per Q6, through a pure, table-tested `map_media(&MessageType)`. The tests cover:
  - the caption vs file name `body`;
  - missing info;
  - encrypted vs plain sources (`has_thumbnail` from either `thumbnail_source`);
  - a sticker staying a placeholder.
- `item_actions` gains the media input: edit only with a caption.
- `FetchMedia`:
  - finds the item by `(room_id, item_id)` in the selected room's timeline (an unknown item gives `ItemNotFound`);
  - applies Q7: semaphore of 4, a 30 s thumbnail and 10 min full deadline, the 10 MiB encrypted-thumbnail fallback rule, the 512 MiB full cap from `info.size`;
  - answers `Media { .. }`. The rules are pure, table-tested helpers (`thumbnail_plan(content)`, `allowed_full(size)`).
- `SaveMedia` writes the bytes to `<path>.tessera-part`, `fsync`s and renames to `path`. A write failure removes the part file and gives `ActionFailed { Media, .. }`. Tested with a tempdir and a fake fetcher (no network).
- `MediaLimits` is sent after login and restore from `load_or_fetch_max_upload_size` (`None` on error).
- Integration tests in `media.rs` (verbatim ignore reason):
  - **plain image:** alice uploads the fixture PNG with `send_attachment_raw` into a plain room with carol. carol's core shows a `Message` with `media.kind == Image`, the width and height, and the file name. `FetchMedia { Full }` returns bytes **identical** to the fixture, and `FetchMedia { Thumbnail }` returns bytes starting with a PNG or JPEG signature (Synapse thumbnail);
  - **encrypted image and file (roadmap acceptance: identical bytes):** under `e2ee_serial` with alice and carol reset, the room is created with `create_room_raw(.., encrypted = true, ..)`. The raw client uploads the fixture image and a 300 KiB random file. carol's `FetchMedia { Full }` gives identical bytes for both. `fetch_raw_event_json` shows each event is `m.room.encrypted`. The raw download of the event's `file.url` (unauthenticated media endpoint via a raw client) differs from the original bytes;
  - **save:** `SaveMedia` of the random file into a tempdir gives a file with identical bytes and no `.tessera-part` left.
- `log_hygiene.rs` adds an encrypted image whose bytes contain the sentinel to its flow. Both the log check and the data-at-rest scan still find no sentinel (Q20).
- Gate, the ignored run and the repeat run of `media` pass. Commit: `phase5.4: media mapping, fetch and save in the core`.

### Task 5: Uploads with local echo and progress in the core (roadmap step 3, core side)
Language: rust
Files: crates/core/src/upload.rs, crates/core/src/media.rs, crates/core/src/timeline.rs, crates/core/src/engine.rs, crates/core/src/sync.rs, crates/core/tests/media.rs, PROGRESS.md
Depends on: Task 4
Acceptance criteria:
- `SendAttachment` follows Q9 (core side):
  - it reads `Path` sources with `tokio::fs::read`;
  - builds `AttachmentConfig` (info, the caller's thumbnail as `Thumbnail`, `reply`) through a pure, unit-tested `attachment_config(&OutgoingAttachment)`;
  - sends with `send_attachment(..).use_send_queue()`.
  - An invalid MIME string, a size over `MediaLimits`, or an unjoined room gives `ActionFailed { Upload, NotAllowed }` before any request.
- `enable_upload_progress(true)` is set after login and restore. `map_send_state` maps `NotSentYet { progress: Some(..) }` to `details.media.upload = Some(UploadProgress { sent, total })` (pure table test).
- Integration tests added to `media.rs`:
  - **round trip through the core, plain and encrypted (roadmap acceptance):** alice's core sends the fixture image with a 64x48 thumbnail, and a random 300 KiB file. alice sees a local echo with `media` before `Sent`. carol's `FetchMedia { Full }` gives identical bytes; carol's `FetchMedia { Thumbnail }` gives exactly the thumbnail bytes alice supplied in the encrypted room (the SDK encrypts and decrypts the thumbnail);
  - **progress:** a 4 MiB random file shows at least one `upload: Some(..)` with `sent < total` on alice's echo, then `Sent`;
  - **refusals:** a file over the advertised limit (a fake limit set in the test core config) is refused with `NotAllowed` and nothing is sent;
  - **discard:** `DiscardSend` on an echo queued behind a running upload removes it.
- Gate, the ignored run and the repeat run of `media` pass. Commit: `phase5.5: attachment uploads with local echo and progress in the core`.

### Task 6: Notifications, notification settings and unread totals in the core (roadmap step 4, core side)
Language: rust
Files: crates/core/src/notifications.rs, crates/core/src/engine.rs, crates/core/src/sync.rs, crates/core/src/room_list.rs, crates/core/src/lib.rs, crates/core/tests/notifications.rs, PROGRESS.md
Depends on: Task 5
Acceptance criteria:
- `notifications.rs` registers the handler once per client and implements Q10's `classify` as a pure function, table-tested for:
  - own sender dropped;
  - old events dropped;
  - duplicates dropped;
  - a highlight tweak giving `Mention`;
  - an encrypted event giving `Encrypted` with the fixed preview;
  - an invite for the user giving `Invite`;
  - a state event dropped;
  - a preview capped at 120 characters, with spoilers replaced.
- `SetRoomNotificationMode`, `ResetRoomNotificationMode`, `SetDefaultNotificationMode`, `AddKeyword` and `RemoveKeyword` call the Q11 SDK methods, with keyword validation (pure test). `NotificationSettings` events are sent after login and on change. `RoomSummary.notification_mode` comes from `cached_user_defined_notification_mode()`.
- `UnreadTotals` is computed over all joined, non-muted rooms, at most once per second and only on change (pure `totals(rooms)` table test plus a throttle test with paused tokio time).
- Integration tests in `notifications.rs` (verbatim ignore reason; `reset_notification_baseline` for alice and carol first; roadmap acceptance "mention in a muted room, DM, keyword"):
  - **DM:** alice sends to a DM with carol; carol's core emits `Notification { is_dm: true, kind: Message, sender: alice }` with the preview;
  - **mention in a muted room:** carol sets the group room to `Mute`. alice mentions carol there, then sends a sentinel in the DM. carol gets the DM sentinel's notification and **no** notification for the mention (Q19 bounding). After `ResetRoomNotificationMode`, the next mention gives `kind: Mention`;
  - **keyword:** carol adds `tessera-kw-<hex>` and sets the room to `MentionsAndKeywordsOnly`. A plain message from alice gives no notification (bounded as above), and a message containing the keyword gives `kind: Mention`;
  - **own messages:** carol's own message never gives a notification;
  - **invite:** alice invites carol to a new room; carol gets `kind: Invite`;
  - **totals:** carol's `UnreadTotals.notifications` goes up after alice's DM message and returns to 0 after `MarkRead`.
- Gate and the ignored run pass, and the `notifications` binary passes twice in a row. Commit: `phase5.6: notifications, notification settings and unread totals in the core`.

### Task 7: Rooms, members, profile and spaces in the core (roadmap step 5, core side)
Language: rust
Files: crates/core/src/rooms.rs, crates/core/src/spaces.rs, crates/core/src/room_list.rs, crates/core/src/engine.rs, crates/core/src/lib.rs, crates/core/tests/rooms.rs, PROGRESS.md
Depends on: Task 6
Acceptance criteria:
- Pure, table-tested helpers:
  - `create_room_request(form)` (presets, the encryption `initial_state`, alias local-part validation, name 1-255);
  - `parse_room_target(&str)`: `#alias:server`, `!id:server`, `matrix.to` links with `via`, and junk refused;
  - `role_of(UserPowerLevel)` (`Infinite` → `Creator`; 100+ Admin, 50+ Moderator);
  - `permissions(&RoomPowerLevels, own)`;
  - `sort_members`.
- `CreateRoom`, `CreateDm` (reusing an existing DM), `JoinRoom`, `AcceptInvite`, `DeclineInvite`, `InviteUser`, `LeaveRoom` (deselects first), `OpenRoomDetails`/`RoomDetails`, `SetRoomName`, `SetRoomTopic`, `SetRoomAvatar`, `EnableEncryption`, `ListRoomMembers`, `SetDisplayName`, `SetAvatar`, `RemoveAvatar` and `OwnProfile` work per Q13. Failures give `ActionFailed { Rooms | Profile, .. }` with fixed messages.
- Spaces per Q15: the core forwards `Spaces`. `RoomListQuery.space` adds `new_filter_identifiers` (unit test of the filter composition in `room_list.rs`).
- Integration tests in `rooms.rs` (verbatim ignore reason; roadmap acceptance "room create, invite, and leave"):
  - **create encrypted and invite:** alice `CreateRoom { name, topic, encrypted: true, invite: [carol] }` gives `RoomCreated`. alice's list shows the room with `is_encrypted`; carol's list shows it with `is_invite`. carol `AcceptInvite`, and it becomes joined with the name and topic;
  - **join by alias:** alice creates a public room with alias `tessera-<hex>`; carol `JoinRoom { "#tessera-<hex>:localhost" }` gives `RoomJoined`;
  - **leave:** carol `LeaveRoom`; the room leaves carol's list and alice's member list no longer has carol as joined;
  - **decline:** an invite declined by carol disappears from her list;
  - **settings:** alice sets the name, topic and a 64x48 avatar, and carol's summary reflects the name and avatar. `EnableEncryption` on a plain room makes `is_encrypted` true for both. carol, without power, gets `RoomDetails.permissions` all false, and `SetRoomName` from her gives `ActionFailed`;
  - **members:** `ListRoomMembers` lists alice as `Creator` or `Admin` and carol as `User`;
  - **DM:** `CreateDm { carol }` twice gives the same room ID;
  - **profile:** alice `SetDisplayName("Alice <hex>")`; carol's member list shows the new name; `OwnProfile` reflects it. The name is restored at the end;
  - **spaces:** a space made by `create_space_raw` with one child room: alice's `Spaces` lists it at level 0. `SetRoomListQuery { space: Some(id) }` lists exactly the child room.
  - Created rooms are left at the end of each test.
- Gate and the ignored run pass. Commit: `phase5.7: rooms, members, profile and spaces in the core`.

### Task 8: Threads in the core (roadmap step 6, core side)
Language: rust
Files: crates/core/src/threads.rs, crates/core/src/engine.rs, crates/core/src/timeline.rs, crates/core/src/outgoing.rs, crates/core/src/lib.rs, crates/core/tests/threads.rs, PROGRESS.md
Depends on: Task 7
Acceptance criteria:
- `select_room` builds the main timeline with `room.timeline_builder().with_focus(Live { hide_threaded_events: true })`. Every existing timeline test passes unchanged.
- `MessageDetails.thread` is mapped through a pure `map_thread_summary` (counts, latest sender and preview, unknown details), table-tested.
- `OpenThread`/`CloseThread` per Q16, with `ThreadDiff` and `ThreadPagination`. `PaginateBackwards` gains the thread target through the `thread:` prefix rule (pure `route_item` table test). Selecting another room or logging out closes the thread.
- `Send` with `thread_root` goes through the thread timeline.
- Integration tests in `threads.rs` (verbatim ignore reason):
  - **summary and hiding:** alice sends a root in a plain room with carol; carol replies with `thread_root = root`. Within 30 s alice's main timeline shows the root with `thread.replies == 1` and the latest sender carol, and the reply is **not** a main-timeline item;
  - **thread timeline:** alice `OpenThread { root }` receives `ThreadDiff`s containing the root and carol's reply, with item IDs starting with `thread:`. alice sends in the thread, and the summary becomes 2;
  - **actions in a thread:** alice reacts to carol's reply through its `thread:` item ID, and carol's thread timeline shows the reaction;
  - **Element-style thread from a raw client:** a reply sent by a raw SDK client with an `m.thread` relation is counted the same way.
- If the summary never appears with `ThreadingSupport::Disabled`, the task stops and asks (Q16).
- Gate and the ignored run pass. Commit: `phase5.8: thread summaries and thread timelines in the core`.

### Task 9: Media in the timeline in the app: thumbnails, viewer, file cards, save (roadmap steps 1 and 2, UI)
Language: rust
Files: crates/app/src/media.rs, crates/app/src/dialogs.rs, crates/app/src/state.rs, crates/app/src/timeline.rs, crates/app/src/screens/timeline_pane.rs, crates/app/src/screens/media_viewer.rs, crates/app/src/screens/mod.rs, crates/app/src/lib.rs, PROGRESS.md
Depends on: Task 8
Acceptance criteria:
- `media.rs` holds pure, unit-tested functions:
  - `media_box(width, height) -> Size` (Q7: fit 400x300, default 320x180, never zero, never upscaled past the original);
  - `decode_media(bytes, Limits) -> Option<(Handle, w, h)>`, which refuses images over the limits and decodes the first GIF frame (fixture tests: PNG, JPEG, WebP, GIF, a 20000x20000 header refused, truncated bytes refused);
  - a `MediaCache` LRU with a byte budget (eviction order and budget tests);
  - `is_dangerous_extension(name)` (table test, case-insensitive, no extension → dangerous);
  - `suggested_file_name(name)` (path separators, `..`, control and bidi characters removed; empty → "download").
- Row estimates use `media_box`, so a thumbnail arriving changes no row height: an `update` test applies `Media` for a visible row and asserts the measured height is unchanged. The Phase 3 16 ms timing test and layout test still pass.
- Visible-window fetching: only rows in the visible window plus one screen send `FetchMedia { Thumbnail }`, each at most once while the cache holds it (`update` test with the `mpsc` sink).
- Clicking an image opens the full-size viewer overlay (`image::viewer`) and sends `FetchMedia { Full }`. **Save…** and **Close**/Esc are offered. The full image is dropped on close.
- File, video and audio cards show the sanitised name, the size label and the type, with **Save…**:
  - **Save…** goes through a `Dialogs` trait (real: `rfd::AsyncFileDialog::save_file` parented with `window::run`; fake in tests) and then `SaveMedia`;
  - after `MediaSaved`, **Open** appears for non-dangerous names, behind the confirmation bar, and runs `open::that` in a `Task`.
- `Debug` of the app state with a media cache and an open viewer shows no bytes or file names (sentinel test).
- Human checklist gains: thumbnails in plain and encrypted rooms (both themes, no jump while loading); the viewer (zoom, pan, Esc); GIF first frame; file cards; Save dialog on GNOME and KDE Wayland and X11; Open after save; a dangerous file shows no Open.
- Gate passes. Commit: `phase5.9: inline media, viewer and file cards in the timeline`.

### Task 10: Uploads in the app: picker, drag and drop, paste, thumbnails, progress (roadmap step 3, UI)
Language: rust
Files: crates/app/src/upload.rs, crates/app/src/dialogs.rs, crates/app/src/composer.rs, crates/app/src/state.rs, crates/app/src/screens/upload_dialog.rs, crates/app/src/screens/timeline_pane.rs, crates/app/src/screens/mod.rs, crates/app/src/lib.rs, PROGRESS.md
Depends on: Task 9
Acceptance criteria:
- `upload.rs` pure functions, unit-tested (roadmap acceptance "thumbnail generation tests"):
  - `mime_for(name, head_bytes)`: the extension table, a mismatch between extension and image signature sent as a file, unknown → `application/octet-stream`;
  - `make_thumbnail(bytes) -> Option<UploadThumbnail>`:
    - the 1,200x900 fixture gives a JPEG of at most 800x600 with the aspect kept;
    - an image with alpha gives a PNG;
    - an image of 640x480 under 1 MiB gives `None`;
    - an EXIF-rotated JPEG fixture gives a thumbnail with swapped width and height;
    - an over-limit header gives `None` without allocating;
    - output dimensions are never larger than the input;
  - `pasted_image_name(now)`;
  - `check_size(len, limit)`.
- Entry points, each tested through `update` with fakes:
  - an **Attach** button and Ctrl+U open the picker (`Dialogs::pick_files`);
  - `FileHovered` shows a "Drop to upload" layer and `FilesHoveredLeft` hides it; `FileDropped` paths collect into one confirmation (a 300 ms debounce gathers multi-file drops);
  - Ctrl+V in the composer goes through `Binding::Custom` and a `Clipboard` trait (real: arboard, `get_image` then `get_text`; fake in tests): an image opens the confirmation with a PNG; text is inserted as a paste; an arboard error falls back to `iced::clipboard::read`.
- File reading, thumbnailing and PNG encoding run in `Task`s, never in `update`.
- The confirmation dialog lists name, size and preview; refuses over-limit files with the limit shown; carries the EXIF note for JPEG photos; and on **Send** sends one `SendAttachment` per file, in order, with the current reply target and, inside the thread panel, `thread_root`.
- Local echoes with `media.upload` show a progress bar and a **Cancel** button (`DiscardSend`). Failure shows the Phase 3 retry and discard actions.
- Human checklist gains:
  - drag and drop on X11 (one and several files), and confirm **nothing happens on native Wayland** (Q4; recorded gap);
  - paste a screenshot (Ctrl+V) on GNOME Wayland, KDE Wayland and X11;
  - paste text still works;
  - the picker on each desktop;
  - the progress bar on a large file;
  - the received thumbnail looks right in Element Web;
  - the EXIF note wording.
- Gate passes. Commit: `phase5.10: uploads from picker, drag and drop and clipboard with progress`.

### Task 11: Desktop notifications, notification settings UI and tray icon (roadmap step 4, UI)
Language: rust
Files: crates/app/src/notifications.rs, crates/app/src/tray.rs, crates/app/src/state.rs, crates/app/src/screens/main_view.rs, crates/app/src/screens/room_list_pane.rs, crates/app/src/screens/mod.rs, crates/app/src/lib.rs, PROGRESS.md
Depends on: Task 10
Acceptance criteria:
- `should_show` and the 3 s per-room folding (Q10) are pure and table-tested:
  - focused vs unfocused;
  - the selected room;
  - notifications off;
  - content off, giving the "New message" / "Mentioned you" / "Invited you" bodies;
  - a burst folding into "N new messages";
  - names and previews sanitised.
- Notifications are sent through a `Notifier` trait (real: notify-rust in a `Task`; Linux "default" action → `Message::OpenRoomFromNotification(room_id)` → select and `gain_focus`; fake in tests). Mentions also return `request_user_attention`.
- Room header menu: the four Q11 choices send the right command once each. Room list rows show the muted mark from `notification_mode`.
- Tray:
  - `tray.rs` draws the two icon variants (pure: RGBA size and a red pixel at the dot position only in the badge variant);
  - `tray_tooltip(totals)` gives "Tessera", "Tessera – 3 unread", "Tessera – 99+ unread";
  - the tray is created on the first window-opened message, and creation failure is tolerated (fake test);
  - tray **Show** and **Quit** map to `gain_focus` and the existing close path (`update` tests);
  - `UnreadTotals` updates the icon only when the variant or tooltip changes.
- Human checklist gains:
  - notifications on GNOME and KDE for a DM, a mention and a keyword while unfocused, and none while focused on that room;
  - "Show message content" off hides the text;
  - clicking a notification opens the room (Linux);
  - the tray icon and red dot on KDE, and on GNOME with the AppIndicator extension (note what plain GNOME shows);
  - the tooltip count; Show and Quit;
  - muting a room silences it in Tessera and in Element Web.
- Gate passes. Commit: `phase5.11: desktop notifications, room notification modes and tray badge`.

### Task 12: Room dialogs, room settings, member list and pills (roadmap step 5 UI; Phase 3 and 4 deferrals)
Language: rust
Files: crates/app/src/rooms.rs, crates/app/src/links.rs, crates/app/src/rich_text.rs, crates/app/src/state.rs, crates/app/src/screens/room_dialogs.rs, crates/app/src/screens/room_settings.rs, crates/app/src/screens/members.rs, crates/app/src/screens/main_view.rs, crates/app/src/screens/room_list_pane.rs, crates/app/src/screens/timeline_pane.rs, crates/app/src/screens/mod.rs, crates/app/src/lib.rs, PROGRESS.md
Depends on: Task 11
Acceptance criteria:
- **Create room** dialog per Q13 (Encrypted on by default). Its validation is a pure, tested `CreateRoomForm::validate`. **Create** sends `CreateRoom` once, and `RoomCreated` selects the room when it appears (`update` test with a later `RoomListChanged`).
- **Start a DM**, **Join a room** (alias, ID or link through the same parser as the core's, mirrored in a pure app helper with the same table) and **Accept**/**Decline** on invite rows send their commands once each.
- Room settings panel: fields enabled from `RoomDetails.permissions`. The avatar goes through the picker and an off-thread 512x512 downscale (pure test of the scaling). **Enable encryption** sits behind a warning card. **Leave** is behind a confirmation whose warning matches Q13 (pure `leave_warning(details, members)` test).
- Member list panel: sorted by role then name, with a role badge, the sanitised name and the elided ID, plus **Invite** (user ID validated in the app before sending). `truncated` shows "Showing the first 5,000 members".
- **Pills (Q14):**
  - `pill_label(target, members, rooms)` table test: a known member gives the current display name; an unknown member gives the full ID; a sender-chosen label is never used; a long ID is elided in the middle; a room in the list gives its name, else the alias or ID;
  - the timeline renders labels through it, with an ID tooltip;
  - a user pill opens the user card with **Message**, which sends `CreateDm`;
  - a room pill of an unjoined room shows the "Join …?" bar, and **Join** sends `JoinRoom` once.
- Every new overlay passes the fixed-tree tag test, and `Debug` of each new state shows no topic, keyword or member names beyond counts (sentinel).
- Human checklist gains:
  - create an encrypted room and see the lock and Element Web's encryption shield;
  - create a public room with an alias and join it from Element Web;
  - join by alias from Tessera;
  - accept and decline invites from Element Web;
  - invite, leave and the leave warnings;
  - room settings edits seen in Element;
  - the member list with an Element-created room's power levels;
  - pills labelled from the target (a message from Element Web whose pill text names someone else shows the real user);
  - the user card and **Message**;
  - joining from a room pill.
- Gate passes. Commit: `phase5.12: room dialogs, room settings, member list and target-labelled pills`.

### Task 13: Spaces sidebar and thread panel (roadmap steps 5 and 6, UI)
Language: rust
Files: crates/app/src/spaces.rs, crates/app/src/threads.rs, crates/app/src/state.rs, crates/app/src/timeline.rs, crates/app/src/screens/spaces_sidebar.rs, crates/app/src/screens/thread_panel.rs, crates/app/src/screens/timeline_pane.rs, crates/app/src/screens/main_view.rs, crates/app/src/screens/mod.rs, crates/app/src/lib.rs, PROGRESS.md
Depends on: Task 12
Acceptance criteria:
- **Spaces sidebar:**
  - **Home** plus one button per level-0 space from `Spaces`, with level-1 children under the selected one;
  - selecting sends `SetRoomListQuery` with `space` and keeps the tab and search (`update` test);
  - a space that vanishes from `Spaces` resets the selection to Home.
- **Thread summary row:** "N replies · <name>: <preview>" under the root, clickable.
- **Thread panel:**
  - opening sends `OpenThread` once and shows the panel; `ThreadDiff`s apply to a separate `TimelineModel` instance (the Phase 3 diff tests are reused on it);
  - the panel composer sends `Send` with `thread_root`;
  - actions on panel items use their `thread:` IDs;
  - Esc or × sends `CloseThread`;
  - selecting another room closes the panel.
- The main timeline's scroll position and the main composer's draft are unchanged by opening and closing the panel (fixed-tree tag test, plus an `update` test on the scroll state).
- Human checklist gains:
  - the spaces sidebar with spaces made in Element Web (icons, tooltips, filtering, Home);
  - thread summaries match Element Web's counts;
  - the thread panel opens, scrolls and sends, and Element Web shows the reply in the thread;
  - threaded replies no longer appear in the main timeline.
- Gate passes. Commit: `phase5.13: spaces sidebar and thread side panel`.

### Task 14: Local settings storage in the core
Language: rust
Files: crates/core/src/paths.rs, crates/core/src/api/types.rs, PROGRESS.md
Depends on: Task 13
Acceptance criteria:
- `DataPaths::settings_file()`, `read_settings() -> UiSettings` and `write_settings(&UiSettings)`:
  - the write is atomic (temp file plus rename);
  - a missing file gives defaults;
  - a corrupt file gives defaults and is renamed `settings.json.bad` (unit tests with a tempdir).
- `UiSettings { theme: ThemeChoice { System, Light, Dark }, desktop_notifications: bool, show_message_content: bool, tray_badge: bool }` defaults to System/true/true/true, and unknown JSON fields are ignored (forward compatibility test).
- The Phase 4 Q16 (c) data-root test is updated to allow exactly `settings.json`, and it still fails on any other file (seen red by adding a stray file in the test).
- `scripts/check-core-api.sh` exits 0 (`paths` stays SDK-free).
- Gate passes. Commit: `phase5.14: local ui settings file in the data root`.

### Task 15: Settings screen (roadmap step 7)
Language: rust
Files: crates/app/src/settings.rs, crates/app/src/state.rs, crates/app/src/theme.rs, crates/app/src/main.rs, crates/app/src/screens/settings.rs, crates/app/src/screens/main_view.rs, crates/app/src/screens/mod.rs, crates/app/src/lib.rs, PROGRESS.md
Depends on: Task 14
Acceptance criteria:
- The Settings screen opens from the account area and with Ctrl+, and has the Q17 sections:
  - **Profile:** display name with Save (`SetDisplayName`), avatar Change/Remove (picker, off-thread 512x512 downscale, `SetAvatar`/`RemoveAvatar`);
  - **Notifications:** default modes, keywords with add and remove (validated in the app), and the three local toggles;
  - **Appearance:** System, Light or Dark;
  - **Keyboard shortcuts:** the list from one `SHORTCUTS` table also used by the key handling (Task 16), with the Wayland drag-and-drop note;
  - **Push-to-talk:** the placeholder line;
  - **Sessions:** a link to the existing screen.
- Local settings are read at boot through `paths` (before login, so the theme applies on the login screen) and written in a `Task` on each change. `theme_for` honours the override over the system mode (pure test).
- `update` tests: each control sends its command once or writes the settings once; a keyword over 64 characters is refused; turning desktop notifications off stops `Notifier` calls (fake).
- Human checklist gains: the Settings layout in both themes; display name and avatar changes seen in Element Web; theme override (System/Light/Dark) survives a restart; notification toggles take effect; the push-to-talk placeholder wording.
- Gate passes. Commit: `phase5.15: settings screen with profile, notifications, theme and shortcuts`.

### Task 16: Accessibility pass: room switcher and keyboard navigation (roadmap step 8)
Language: rust
Files: crates/app/src/switcher.rs, crates/app/src/keys.rs, crates/app/src/state.rs, crates/app/src/room_list.rs, crates/app/src/screens/switcher.rs, crates/app/src/screens/main_view.rs, crates/app/src/screens/mod.rs, crates/app/src/lib.rs, docs/accessibility.md, PROGRESS.md
Depends on: Task 15
Acceptance criteria:
- `switcher.rs`: a pure `rank_rooms(query, rooms) -> Vec<RoomId>` ranks prefix matches first, then word-start, then substring. It is case- and accent-insensitive through the same normalisation as the composer search, capped at 20, and table-tested. A query with fewer than 3 local matches also sends `SetRoomListQuery` search to the core and merges the results.
- `keys.rs`: one `SHORTCUTS` table and a pure `shortcut_for(key, modifiers, platform) -> Option<Shortcut>`, table-tested for Ctrl vs Cmd on macOS, Alt+↑/↓, Ctrl+Shift+↑/↓ (next unread), Ctrl+K, Ctrl+U, Ctrl+, and Esc. Shortcuts typed inside the composer don't steal its own keys (Enter, ↑ for edit-last, Tab in autocomplete): a table test with the composer focused.
- Switcher overlay: Ctrl+K opens it with the field focused; ↑/↓ move, Enter selects the room and closes, Esc closes and gives focus back to the composer (`update` tests).
- Tab and Shift+Tab cycle the fixed focus order through `focus_next`/`focus_previous`, and every overlay closes with Esc (`update` tests per overlay).
- `docs/accessibility.md` records:
  - the keyboard map;
  - the screen-reader gap (no AccessKit in iced 0.14, as verified; the upstream issue to watch, re-checked at each iced upgrade);
  - the Wayland drag-and-drop gap;
  - high-contrast notes for both themes.
- Human checklist gains:
  - a keyboard-only session: switch rooms with Ctrl+K and Alt+↑/↓, reply, attach a file, open and close a thread, open Settings, all without the mouse;
  - focus is visible at each step in both themes;
  - **IME (deferred from Phase 3):** ibus or fcitx composition in the composer, where Enter commits the candidate without sending, the candidate window sits near the caret, and Enter after commit sends.
- Gate passes. Commit: `phase5.16: room switcher, keyboard navigation and accessibility notes`.

### Task 17: Phase 5 wrap-up
Language: rust
Files: crates/app/src/state.rs, crates/app/tests/no_secret_logging.rs, PROGRESS.md
Depends on: Task 16
Acceptance criteria:
- An app-side redaction sweep: one test builds an `App` in each of these states with sentinel values and asserts no sentinel in `format!("{:?}", app)` or in any `Message` `Debug`:
  - the media viewer and cache;
  - the upload confirmation with a pasted image;
  - a pending notification;
  - the room settings with a topic;
  - the member list;
  - the thread panel with a draft;
  - the Settings screen with keywords;
  - the switcher with a query.
- `no_secret_logging.rs` gains the new sensitive type names (media bytes, notification previews, file names, keywords) in its pattern list.
- PROGRESS.md:
  - marks the Phase 5 agent work done (the human checkpoint stays open);
  - consolidates "Phase 5 open issues (summary)": the Q21 scope cuts, the Q4 Wayland drag-and-drop gap, the Q2 untested Windows/macOS code, the Q20 residuals, every Low or Medium review finding, and the test durations;
  - adds a short "Phase 5 security notes" section (media at rest, notification content, saved files, EXIF).
- The full ignored suite passes after `scripts/prune-test-rooms.sh`, and again right after it, with both durations recorded (under 25 minutes each, else stop and ask; raised from 15 on 2026-10-11).
- Gate passes. Commit: `phase5.17: phase 5 wrap-up`.

### Phase 5 human checkpoint (collected in PROGRESS.md)
- [ ] Phase 4 sign-off recorded (Q1). Confirm the Q3 dependencies and versions, the ksni licence exception, Q2 (Linux-only checks, no CI gate, Windows/macOS code uncompiled) and Q4 (no drag and drop on native Wayland); revisit any of Q1-Q21.
- [ ] Images: thumbnails inline in plain and encrypted rooms, in both themes, with no scroll jump while loading; click opens the viewer (zoom, pan, Esc); a GIF shows its first frame.
- [ ] Files, video, audio: cards read well; **Save…** opens the native dialog (GNOME Wayland, KDE Wayland, X11) and the saved file matches the original; **Open** after saving plays a video in the system player; a dangerous file offers no Open.
- [ ] Uploads: picker; drag and drop on X11 (one and several files); confirm nothing happens when dropping on a native Wayland window (recorded gap); Ctrl+V pastes a screenshot on GNOME Wayland, KDE Wayland and X11 and text paste still works; the progress bar on a large file; the EXIF note wording; the received image and thumbnail look right in Element Web.
- [ ] Notifications (roadmap): a DM, a mention and a keyword notify while Tessera is unfocused, on GNOME and KDE; nothing for the open room while focused; a muted room stays silent; "Show message content" off hides the text; clicking opens the room.
- [ ] Tray badge (roadmap): the icon and red dot on KDE, and on GNOME with the AppIndicator extension (note plain GNOME's behaviour); the tooltip count; Show and Quit.
- [ ] Rooms: create an encrypted room (lock in Tessera, shield in Element Web); a public room with an alias joined from Element Web; join by alias; accept and decline invites; invite; leave with its warnings; room settings edits seen in Element; the member list with power levels.
- [ ] Pills: labels come from the target (an Element Web message whose pill text names someone else shows the real user); the user card and **Message**; joining from a room pill.
- [ ] Spaces sidebar with spaces made in Element Web; Home; filtering.
- [ ] Threads: summaries match Element Web; the panel opens, scrolls and sends; Element Web shows the reply in the thread; threaded replies are gone from the main timeline.
- [ ] Settings: layout in both themes; display name and avatar changes seen in Element Web; the theme override survives a restart; notification toggles work; the push-to-talk placeholder wording.
- [ ] Keyboard-only navigation (roadmap): Ctrl+K, Alt+↑/↓, Tab order, Esc on every overlay, attach and thread without the mouse; focus visible in both themes.
- [ ] IME (deferred from Phase 3): ibus/fcitx composition in the composer behaves (Enter commits without sending, candidate window near the caret, Enter after commit sends).
- [ ] Read `docs/accessibility.md` and the Phase 5 security notes; accept or reject the residuals (EXIF in originals, notification text in the OS, saved files in clear, no re-notify after late decryption, a lying `info.size`).
