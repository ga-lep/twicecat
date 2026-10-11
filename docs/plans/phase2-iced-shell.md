# Phase 2 Iced Shell and Room List Plan
Status: APPROVED

## Goal
Turn `twicecat-app` into a working desktop shell on top of the approved Phase 1 core API. At startup the app builds a dedicated tokio runtime and starts `Core` on it. Commands go down the core's `mpsc` channel with non-blocking sends from `update`. Core events come back through an Iced `Subscription` that turns each `CoreEvent` into an app `Message`. `update` never awaits network work and never touches an SDK type. The room list and the selected room's timeline are applied diff by diff (`ListDiff`, the SDK-free mirror of all 11 `VectorDiff` variants) to plain `Vec`s in app state. Lists are never rebuilt. The core API grows additively, with no breaking change to the approved contract:
- richer `RoomSummary` view models: avatar ID, DM/space/invite flags, highlight count, marked-unread, a redacted last-message preview;
- a room-list query (tabs DMs / Rooms / Spaces / All plus a search filter, built on the SDK's room-list filters);
- room-list paging;
- on-demand avatar thumbnails through the SDK media API, cached on disk in the SDK's encrypted per-session media store.

The UI has:
- a login screen (server, username, password, plus an OIDC button that opens the browser);
- a loading/sync screen;
- a two-pane main layout. The room-list pane has tabs, search, badges, avatars with an initials placeholder, and windowed rendering. The right pane is minimal: a header, the last messages as plain text, and a single-line composer. Phase 3 replaces it.

Light and dark palettes follow the system setting. The agent proves this work with unit tests on `update` (every diff variant), a deterministic 2,000-room / 16 ms batch test, the existing app-dependency boundary check, and homeserver integration tests. Rendering, theme following, browser opening and live-sync responsiveness are left to the human checkpoint.

### Facts verified while planning (2026-10-01)
Each implementing task re-confirms the APIs it uses (AGENTS.md rule 2) and cites file:line in PROGRESS.md. Registry root: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Locked iced sub-crates: `iced_widget 0.14.2`, `iced_winit 0.14.1`, the rest `0.14.0`. I compiled two probes (`cargo check`) in a scratch copy of the workspace in the session scratchpad, not in the repo: an app `main.rs` using every iced API below, and a core module using every SDK API below. Both compiled with no errors.
- **iced application:** `iced::application(boot, update, view)` (application.rs:79), with `.subscription` :363, `.theme` :378, `.exit_on_close_request` :269, `.window_size` :280 and `.title` :346. `ThemeFn` (:648-672) accepts any `Fn(&State) -> impl Into<Option<Theme>>`. **Returning `None` makes iced use a theme matching the system color scheme.**
- **System theme:** `iced::system::{theme, theme_changes}` (lib.rs:602-605 → iced_runtime system.rs:58 `theme() -> Task<theme::Mode>`, :63 `theme_changes() -> Subscription<theme::Mode>`). `theme::Mode { None, Light, Dark }` (iced_core theme.rs:212). The `ICED_THEME` env var overrides the default theme (theme.rs `Base::default`). `linux-theme-detection` is a default feature of iced (iced Cargo.toml `[features] default`), and iced_winit reads the system theme on Linux only under that feature (iced_winit lib.rs:525). So **following the system needs no new crate**.
- **Custom palettes:** `Theme::custom(name, Palette)` (theme.rs:91). `Palette { background, text, primary, success, warning, danger }` (theme/palette.rs:9-22).
- **Subscriptions and channels:** `Subscription::run_with(data: D: Hash + 'static, builder: fn(&D) -> S)` (iced_futures subscription.rs:198), `run` :182, `batch` :212, `map` :282. `iced::futures` is re-exported (lib.rs:484), so `futures::stream::unfold` turns a tokio `mpsc::Receiver` into a `Stream` with no `tokio-stream` dependency.
- **Window, exit, tasks:** `iced::window::close_requests()` (iced_runtime window.rs:260), `iced::exit()` (iced_runtime lib.rs:124), `Task::{none, perform :48, future :258, batch :90}`, `iced::clipboard::write` (iced_runtime clipboard.rs:49).
- **Widgets** (iced_widget helpers.rs): `column` :480, `row` :539, `container` :237, `scrollable` :1046, `button` :1074, `text` :1143, `text_input` :1444, `tooltip` :1110, `mouse_area` :2010, `space` :1748, and `image` :1812 behind the `image` feature.
  - `text_input`: `.secure(true)` :163, `.on_input` :172, `.on_submit` :194.
  - `scrollable`: `.on_scroll(Fn(Viewport) -> Message)` :164, `Viewport::absolute_offset` :1739, `Viewport::bounds` :1775.
  - `image::Handle::from_bytes(impl Into<Bytes>)` (iced_core image.rs:136), image widget `.width/.height` (iced_widget image.rs:91/:97).
- **iced features:**
  - `image = ["image-without-codecs", "image/default"]`, where `image-without-codecs = ["iced_widget/image", "dep:image"]` and iced's `image` dep is `default-features = false`. With `image-without-codecs`, `iced::widget::image` still resolves through the `iced_widget::*` glob (the probe compiled).
  - Full `image` adds **62** crates to Cargo.lock, including the AV1 encoder rav1e/ravif, rayon, exr, tiff, gif and a second `rand`.
  - `image-without-codecs` plus a direct `image = { version = "0.25", default-features = false, features = ["png", "jpeg", "webp"] }` (unified with iced's `image 0.25.10`) adds **13**: byteorder-lite, fdeflate, image 0.25.10, image-webp 0.2.4, kamadak-exif 0.6.1, miniz_oxide 0.8.9, moxcms 0.8.1, mutate_once 0.1.2, png 0.18.1, pxfm 0.1.30, quick-error 2.0.1, zune-core 0.5.3, zune-jpeg 0.5.15.
  - iced's `tokio` feature only selects iced's own executor and isn't needed.
- **`open` 5.4.4** (MIT; adds is-wsl 0.4.0 and is-docker 0.2.0, both MIT): `open::that_detached(path) -> io::Result<()>` (lib.rs:285). On Windows it tries powershell, then explorer.exe with option-safe argument quoting, then cmd (windows.rs:66-92).
- **`cargo deny check`** with the current, unchanged `deny.toml`, on a lock with tokio (already in the tree), the 13 image crates and `open`: `advisories ok, bans ok, licenses ok, sources ok`. `git diff`-style comparison of the lock: packages added only, no existing version changed.
- **matrix-sdk-ui 0.19.1 room list** (room_list_service/room_list.rs):
  - `entries_with_dynamic_adapters(page_size)` :139: **every `set_filter` yields a `Reset` followed by updates**. The results are sorted by latest event, then recency, then name.
  - `RoomListDynamicEntriesController::{set_filter :342, add_one_page :356, reset_to_one_page :375}`. **Only `page_size` rooms (500 today) are listed until `add_one_page`.**
  - `RoomList::loading_state() -> Subscriber<RoomListLoadingState>` :119, with `RoomListLoadingState::{NotLoaded, Loaded { maximum_number_of_rooms: Option<u32> }}` :282.
  - Room-info notable updates (other than recency alone) produce `VectorDiff::Set` for that room (:245-254), so a newly computed latest event updates the list.
- **Filters** (`room_list_service::filters`, mod.rs:87-104):
  - `new_filter_all`, `new_filter_not`, `new_filter_non_left`;
  - `new_filter_category(RoomCategory::{People, Group})`, which is based on `Room::is_dm()` (category.rs:30-46);
  - `new_filter_space` (space.rs:23);
  - `new_filter_normalized_match_room_name(&str)` (normalized_match_room_name.rs:48) and `new_filter_fuzzy_match_room_name` (:47);
  - `BoxedFilterFn = Box<dyn Filter + Send + Sync>` (:115).
- **Room accessors:**
  - matrix-sdk-base room/mod.rs: `is_space` :156, `unread_notification_counts()` :181 (`UnreadNotificationsCount { highlight_count, notification_count }`, sync.rs:264-270), `num_unread_mentions` :206, `avatar_url() -> Option<OwnedMxcUri>` :240, `is_marked_unread` :582;
  - `Room::state() -> RoomState { Joined, Left, Invited, Knocked, Banned }` (room/state.rs:23/:31, re-exported as `matrix_sdk::RoomState`);
  - `clone_info().heroes()` (room_info.rs:91/:990), where `RoomHero.avatar_url` is in display_name.rs:398;
  - matrix-sdk `Room::is_dm()` (room/mod.rs:4737; sync and approximate).
- **Latest event:**
  - `matrix_sdk_ui::timeline::RoomExt::latest_event(&Room) -> LatestEventValue` (timeline/traits.rs:65/:85), whose variants are `None | Remote { timestamp, sender, is_own, profile, content } | RemoteInvite {..} | Local { .., content, state }` (timeline/latest_event.rs:27). `TimelineItemContent::as_message()` is at content/mod.rs:279 and `Message::body()` at message.rs:83.
  - The base `Room::latest_event()` has the same name. Call the UI one as `RoomExt::latest_event(room)` (as the probe does).
  - **Latest events are lazy.** Nothing is computed for a room until `client.latest_events().await.listen_to_room(room_id)` is called (matrix-sdk latest_events/mod.rs module doc and :143; `Client::latest_events` client/mod.rs:3630). `RoomListService` registers only rooms it subscribes to (room_list_service/mod.rs:522).
- **Media:**
  - `client.media().get_media_content(&MediaRequestParameters { source: MediaSource::Plain(mxc), format }, use_cache)` (media.rs:431);
  - `MediaFormat::{File, Thumbnail(MediaThumbnailSettings::new(UInt, UInt))}` (base media/mod.rs:46/:99/:130), and `Room::avatar(MediaFormat)` (room/mod.rs:577) does the same with `use_cache = true`;
  - `OwnedMxcUri: From<&str>` and `is_valid()` (ruma-common 0.20.0 mxc_uri.rs:51);
  - **the media cache lives in the SQLite media store that `sqlite_store(path, passphrase)` opens with the same path and passphrase as the state store** (client/builder/mod.rs:801-820). It is on disk, encrypted, per session, and removed with the session dir at logout.
- **Test helpers available:** `Client::create_dm(&UserId)` (client/mod.rs:1964), `Client::create_room(Request)` :1931 (a space via `creation_content` `type: m.space`), `Room::upload_avatar` (room/mod.rs:3040).
- **Current repo state:**
  - `crates/app` is a bin-only crate with `iced = "=0.14.0"` and `twicecat-core`.
  - `Core::start(CoreConfig, Arc<dyn SecretStore>) -> CoreHandle { commands: mpsc::Sender<Command> (cap 32), events: mpsc::Receiver<CoreEvent> (cap 256) }` must run inside a tokio runtime (engine.rs:72).
  - `forward_room_list` (engine.rs:195) keeps the controller local, so filters and paging can't be changed today.
  - `RoomSummary` derives `Debug` and is built literally in `room_list.rs:41` and the CLI tests (twicecat-cli.rs:807). The CLI matches `CoreEvent` exhaustively (twicecat-cli.rs:598).
  - `scripts/check-app-deps.sh` already runs in the `lint` CI job.
- **No rule-8 blocker.** Every API the roadmap names for this phase exists in the pinned versions: the room list's built-in filters, the media API, the `Subscription`, and the `image` widget.

### Decisions (adopted under the human's standing pre-authorization, 2026-10-01; revisit at checkpoint)
1. **Q1: new app dependencies.** RECOMMENDED, and adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
   - `crates/app/Cargo.toml` gets:
     - `iced = { version = "=0.14.0", features = ["image-without-codecs"] }` (pin unchanged);
     - `image = { version = "0.25", default-features = false, features = ["png", "jpeg", "webp"] }`, which only turns codecs on in the `image` crate iced already uses;
     - `tokio = { version = "1", features = ["rt-multi-thread", "sync", "time"] }` (already in the tree);
     - `open = "5"` (5.4.4).
   - Caret ranges, locked by Cargo.lock, like Phase 1's small deps.
   - Rejected:
     - iced `image` (62 crates, AV1 encoder);
     - `webbrowser` (more platform deps);
     - a hand-rolled `xdg-open`/`open`/`cmd start` launcher (Windows quoting and injection risk);
     - `im` (Q5);
     - `dark-light` (iced 0.14 already follows the system);
     - iced `tokio` (Q3);
     - `tokio-stream` (`futures::stream::unfold` is enough).
2. **Q2: deny.toml.** RECOMMENDED: no change (`cargo deny check` passes with the new crates). Adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint). Rejected: pre-emptive allowlist additions.
3. **Q3: who owns the core runtime.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
   - The app's `bridge` module builds a dedicated `tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("twicecat-core")` runtime at startup and calls `Core::start` under `rt.enter()`.
   - The app state owns the runtime and shuts it down with `shutdown_timeout(5 s)` after `Shutdown`.
   - Iced keeps its default executor; tokio `mpsc` channels work across executors.
   - Rejected: iced's `tokio` executor shared with the core (couples UI scheduling to SDK load, against the roadmap's "own runtime"), and starting the runtime inside a subscription stream (hidden global; drop-in-async-context hazards).
4. **Q4: the bridge.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
   - The receiver sits in `EventsSlot(Arc<Mutex<Option<Receiver<CoreEvent>>>>)`, whose `Hash` hashes a constant, and is exposed via `Subscription::run_with(slot, events_stream)`. `events_stream` takes the receiver once (a second build yields an empty stream) and yields `BridgeEvent::Core(CoreEvent)`, then `BridgeEvent::Closed` when the channel closes.
   - `update` sends commands with `Sender::try_send`. If the channel is full, it falls back to `Task::future(sender.send(cmd))` (a channel send, not network work). It never uses `blocking_send` and never awaits in `update`.
   - Rejected: `Subscription::run` with a global, and `blocking_send`.
5. **Q5: list state.** RECOMMENDED: a plain `Vec` per list, plus the existing `ListDiff::apply`. Adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint). An additive `ListDiff::apply_owned(self, &mut Vec<T>)` (move instead of clone) may be added in `api/diff.rs` if Task 7's timings need it, with the same 11 variant tests. Rejected: `im::Vector` (new dep; the roadmap allows `Vec`).
6. **Q6: additive core API.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint). No existing variant, field or type changes meaning; only additions:
   - Types (`api/types.rs`):
     - `AvatarId` (string newtype holding an MXC URI);
     - `AvatarImage` (bytes newtype; `Debug` shows only the length);
     - `MessagePreview { sender: UserId, sender_name: Option<String>, body: String, timestamp_ms: u64, is_own: bool }` (`Debug` redacts `body`);
     - `RoomListTab { All, People, Rooms, Spaces }` (`Default = All`) and `RoomListQuery { tab: RoomListTab, search: String }` (`Default`).
   - `RoomSummary` gains:
     - `avatar: Option<AvatarId>`;
     - `is_dm: bool`, `is_space: bool`, `is_invite: bool`;
     - `highlights: u64`, `marked_unread: bool`;
     - `last_message: Option<MessagePreview>`.

     It also gets a `RoomSummary::new(id, name)` constructor with zero/empty defaults, and a manual `Debug` that redacts the preview body.
   - `Command` gains `SetRoomListQuery(RoomListQuery)`, `LoadMoreRooms` and `FetchAvatars(Vec<AvatarId>)`.
   - `CoreEvent` gains `RoomListLoaded { total_rooms: Option<u32> }` and `Avatar { id: AvatarId, image: Option<AvatarImage> }`, where `None` means unavailable.
   - Rejected: replacing the API with the roadmap's illustrative enums, and `#[non_exhaustive]` (that would change the approved contract).
7. **Q7: tabs and search.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint). Filters are built in the core from the SDK's filters. Each one is `new_filter_all` of `non_left` plus:
   - All: nothing more;
   - People: `category(People)` and `not(space)`;
   - Rooms: `category(Group)` and `not(space)`;
   - Spaces: `space`.

   A trimmed, non-empty search adds `normalized_match_room_name` (case- and diacritic-insensitive substring). A new query also calls `reset_to_one_page()`. Rejected: filtering in the app (the roadmap says use the SDK's filters), the fuzzy matcher (surprising matches), and People/Rooms tabs without `not(space)`.
8. **Q8: paging.** RECOMMENDED: keep page size 500 (`DEFAULT_ROOM_LIST_PAGE_SIZE`). The app sends `LoadMoreRooms` when the last rendered row is within 50 rows of the end and `total_rooms` is greater than the loaded count, at most once per loaded length. Adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint). Rejected: page size 10,000 (slower first `Reset` on huge accounts).
9. **Q9: badges.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
   - The core computes `highlights = max(num_unread_mentions, unread_notification_counts().highlight_count)`. Client-side counts can lag in encrypted rooms, and server counts can be imprecise there, so the max never hides a mention.
   - App badge rule, as a pure function: `highlights > 0` gives a highlight badge with that count; else `unread_notifications > 0` gives a count badge; else `unread_messages > 0 || marked_unread` gives a dot; else no badge. Invites get an "Invite" tag.
   - Rejected: separate server and client fields in the API (more surface, the same UI).
10. **Q10: last-message preview.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
    - The room-list forwarder calls `latest_events().listen_to_room(id)` once for each room ID it sees (HashSet of registered IDs; cleared when sync stops).
    - It builds the preview from `RoomExt::latest_event`:
      - `Remote`/`Local` whose `content.as_message()` is `Some` give `body`: first line only, control and bidi characters removed, at most 120 chars. `sender_name` comes from the `Ready` profile.
      - `None`, `RemoteInvite` and non-message content give `None`.
    - The preview is untrusted display data, never logged.
    - Rejected: deserializing the base `TimelineEvent` raw JSON ourselves (duplicates SDK logic, misses local echoes).
11. **Q11: avatars.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
    - `avatar` is the room's `avatar_url()`. For a DM without one it falls back to the single hero's `avatar_url` (`clone_info().heroes()`, only when `is_dm()`).
    - The app requests avatars on demand for rendered rows (`FetchAvatars`, deduplicated in the app and in the core).
    - The core fetches `MediaFormat::Thumbnail(96×96)` with `use_cache = true`, falling back to `MediaFormat::File` only if the thumbnail request fails. At most 4 fetches run at once (a `tokio::sync::Semaphore`). Responses over 1 MiB, or with an invalid MXC URI, give `Avatar { image: None }`. The tasks are aborted on logout and shutdown.
    - "Cache on disk" is the SDK's encrypted per-session SQLite media store (see Facts). No plaintext image files are written.
    - Rejected: an own file cache in the data dir (plaintext avatars, eviction to write, duplicates the SDK cache), and eager fetching for every room (thousands of requests at first login).
12. **Q12: rendering a large list.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
    - Fixed `ROW_HEIGHT` rows. The scrollable's `on_scroll` viewport gives the visible index range (a pure function with 10 rows of overscan).
    - Only that window is built, between two `space()` spacers sized to the hidden rows. The same range drives avatar requests and `LoadMoreRooms`.
    - Phase 3's timeline virtualization can reuse the pure function.
    - Rejected: building all 2,000 rows per frame, and iced `lazy` (an extra feature that doesn't cut layout of off-screen rows).
13. **Q13: theme.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
    - Two custom palettes, `Theme::custom("Twicecat Light", ..)` and `Theme::custom("Twicecat Dark", ..)`.
    - The mode comes from `system::theme()` at boot and from `system::theme_changes()`. `Mode::None` maps to light.
    - Badge and highlight colours derive from the palette.
    - `ICED_THEME` still overrides iced's own default; this is documented, not removed.
    - Rejected: `dark-light`, and returning `None` (that would follow the system but with iced's stock palettes, not ours).
14. **Q14: app crate layout.** RECOMMENDED: split `crates/app` into a library (`src/lib.rs` with `bridge`, `state`, `room_list`, `screens`, `theme`, `avatars` modules) and a thin `src/main.rs`, so `tests/` can drive `update` headlessly and the timing test lives next to the code. Adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint). Rejected: bin-only (no integration tests possible).
15. **Q15: right pane scope.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
    - The right pane shows the selected room's name, the last 50 `TimelineItemKind::Message` items as plain `sender: body` text (from `TimelineDiff` applied to a `Vec`), and a single-line `text_input` composer that sends `SendMessage`.
    - Formatting, virtualization, pagination and the real composer are Phase 3.
    - Rejected: a placeholder only (the human couldn't check live updates or sending), and a full timeline (Phase 3 scope).
16. **Q16: app integration tests in CI.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
    - The `integration` CI job and the agent's ignored-test gate become `cargo test -p twicecat-core -p twicecat-app --locked -- --ignored --skip keyring`. Same `#[ignore = "needs local homeserver: docker compose up -d --wait"]` convention, verbatim.
    - The headless app test drives the real bridge and `update` without opening a window.
    - Rejected: app tests only on the human list.
17. **Q17: the 16 ms timing test.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
    - A plain `#[test]` in the normal debug test profile, which runs in every `test` CI job on all 3 OSes. Debug is slower than release, so passing there is conservative.
    - Synthetic rooms are generated deterministically (index-based names, IDs, counts and previews; no RNG).
    - Each batch shape is timed through `update(Message::Core(RoomListChanged(..)))`, including the derived avatar and paging bookkeeping. 15 timed runs per shape, after one warm-up, each on a fresh clone of the start state.
    - The test asserts the **median** is under 16 ms and prints all timings on failure. Median, not max, absorbs CI scheduler noise.
    - If it fails only on a CI runner twice in a row, stop and ask (rule 8).
    - Rejected: a release-only CI step (adds a release build of the matrix-sdk tree, with CI disk and time already tight), a `criterion` bench (new dep, not a pass/fail gate), and max-of-runs (flaky on shared runners).
18. **Q18: shutdown on window close.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint). `exit_on_close_request(false)` and `window::close_requests()`. On close the app sends `Shutdown`, shows "Closing…", and calls `iced::exit()` when `BridgeEvent::Closed` arrives, or after 10 s. The timer runs on the core runtime: the `JoinHandle` is awaited in a `Task::future`. The runtime is then shut down with a 5 s timeout. Rejected: immediate exit, which could kill the engine mid-store-write.
19. **Q19: secrets in the UI.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
    - The password field's message and state hold `api::Password` (`on_input(|s| Message::PasswordChanged(Password::from(s)))`; the widget reads `expose()`). The composer holds `api::MessageText`. So a `Debug` of any `Message` or state field never shows a password, a message body or a preview.
    - On submit the password is moved into the command and the field is cleared.
    - The app never logs `Message`s. It calls `twicecat_core::logging::init("warn,twicecat_core=info,twicecat_app=info")`.
    - Residual: iced's own text-input buffers aren't zeroized. This is documented as an open issue.
    - Rejected: plain `String` fields.
20. **Q20: app identity and storage.** RECOMMENDED: the app uses the same `DataPaths::from_os()` and `KeyringSecretStore::new(KEYCHAIN_SERVICE)` as the CLI, so a session made in one restores in the other. Running both at once on one data dir is unsupported (SQLite store lock); this is documented in PROGRESS.md and the app's `--help`. Adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint). Rejected: a separate app identity (two logins, two devices).
21. **Q21: OIDC button.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint):
    - The button sends `LoginOAuth { server }` using the server field.
    - On `OAuthUrl(url)` the app re-checks the URL (scheme `https`, or `http` to `127.0.0.1`/`[::1]`; no userinfo; pure function) and calls `open::that_detached(url.expose())` inside a `Task::perform`, never in `update`.
    - It shows "Continue in your browser" with **Cancel** (`CancelOAuth`). If opening fails, it shows a **Copy link** button (`iced::clipboard::write`).
    - The URL is never logged or displayed in full.
    - Rejected: displaying the URL as text (it carries `state`).
22. **Q22: scope cuts.** RECOMMENDED, adopted under the human's standing pre-authorization (2026-10-01; revisit at checkpoint). Out of scope for Phase 2: the timeline rendering, composer, read receipts and pagination of Phase 3; E2EE flows (Phase 4); creating or joining rooms and accepting invites (Phase 5; invites are only listed and tagged); spaces as a tree (the Spaces tab lists space rooms flat); user avatars in the timeline; multi-account. No changes to existing pins.

### Conventions for every task
- **Gate (rule 4), required before each commit:**
  - `cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && cargo test --workspace --locked`, plus `scripts/check-app-deps.sh`, `scripts/check-core-api.sh` and `cargo deny check`.
  - Tasks that touch the core or the bridge also run `docker compose up -d --wait && scripts/create-test-users.sh && cargo test -p twicecat-core -p twicecat-app --locked -- --ignored --skip keyring` (before Task 6, `-p twicecat-core` only), then `docker compose stop`.
  - The agent never runs the real-keychain test.
- **One commit per task**, message `phase2.<task number>: <summary>`, attribution trailer as instructed by the session. ROADMAP.md is never committed.
- **PROGRESS.md (rule 5).**
  - Task 1 adds a "Phase 2 steps" checklist (one line per task, with the roadmap steps it covers), the new dependencies under "Pinned versions", and a "Phase 2 human checkpoint" list.
  - Each task ticks its line and adds a "Phase 2 / Task N notes" block with:
    - the checks run and their results;
    - every iced, matrix-sdk(-ui), `open` or `image` API used, with the registry file:line where it was confirmed (rule 2);
    - open issues;
    - new human items.
  - Rendering, theme following, browser opening, OS behaviour and responsiveness go on the human list, never ticked by the agent (rule 9).
- **Boundary (rule 7).**
  - `crates/app` depends only on `twicecat-core`, iced, image, tokio and open. It imports only `twicecat_core::{api, Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE, paths, secrets, logging}`.
  - `crates/core/src/api/` stays SDK-free (`check-core-api.sh`). SDK→API conversions stay private in the core.
- **No network or blocking in `update`.** `update` only mutates state, calls `try_send`, and returns `Task`s. Blocking or slow work (opening the browser, timers) lives in `Task`s. Unit tests call `update` directly with a plain `tokio::sync::mpsc::channel` as the command sink and no runtime.
- **Secrets hygiene (rule 6).** No password, token, message body, preview body or full OAuth URL reaches `tracing`, a file, or a `Debug` string. Every new public type that holds one has a redacted `Debug`, unit-tested with a sentinel value.
- **Stop and ask (rule 8)** if any API above behaves differently from this plan, if an integration test or the timing test is flaky twice in a row, or if a step needs a real account.

### Task 1: App dependencies, boundary check hardening, Phase 2 ledger
Language: rust
Files: crates/app/Cargo.toml, Cargo.lock, scripts/check-app-deps.sh, PROGRESS.md
Depends on: none
Acceptance criteria:
- `crates/app/Cargo.toml` has exactly the Q1 additions. `iced` keeps `=0.14.0` and only gains `features = ["image-without-codecs"]`.
- `cargo tree -p twicecat-app --depth 1 -e normal` lists iced v0.14.0, image v0.25.x, tokio v1.x, open v5.x and twicecat-core.
- The `Cargo.lock` diff adds packages only. The 13 image crates plus open, is-wsl and is-docker are listed in PROGRESS.md, and no existing version changes.
- `cargo deny check` passes with `deny.toml` unchanged.
- `scripts/check-app-deps.sh` default pattern becomes `(matrix-sdk|matrix_sdk|ruma|eyeball|imbl)`, so the SDK's helper crates are refused too, and the comment says why.
  - It exits 0 on the tree.
  - Negative self-checks are recorded in PROGRESS.md: a temporary direct `ruma` dep makes it exit 1, `scripts/check-app-deps.sh iced` exits 1, and the manifest is reverted.
  - shellcheck is clean, and the `lint` CI job is unchanged (it already runs the script).
- PROGRESS.md gains the Phase 2 steps list, the pinned-versions lines, and a "Phase 2 human checkpoint" list seeded with:
  - CI green on Linux, Windows and macOS with the new dependencies;
  - "Don't run twicecat-cli and the app on the same data dir at once" (Q20).
- Gate passes (the app still builds; unused deps are allowed for this commit). Commit: `phase2.1: app dependencies and stricter app boundary check`.

### Task 2: Additive core API for Phase 2
Language: rust
Files: crates/core/src/api/mod.rs, crates/core/src/api/types.rs, crates/core/src/room_list.rs, crates/core/src/engine.rs, crates/core/src/bin/twicecat-cli.rs, PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- The API has exactly the Q6 additions, all `Clone + Debug`, SDK-free and documented. `scripts/check-core-api.sh` exits 0.
- `RoomSummary::new(id, name)` gives zero counts, `false` flags and `None` options. `summarize` and the CLI tests use it (or struct update syntax) and still pass.
- Redaction unit tests use sentinel values:
  - `format!("{:?}")` of a `RoomSummary` whose preview body is `secret-body-x` doesn't contain it, and neither does a `CoreEvent::RoomListChanged` carrying it;
  - `AvatarImage(vec![0xAB; 3])` prints its length, not its bytes;
  - `MessagePreview` shows sender and timestamp but not the body.
- The engine accepts the new commands without panicking:
  - `SetRoomListQuery` and `LoadMoreRooms` are no-ops before Task 4;
  - `FetchAvatars(ids)` emits `Avatar { id, image: None }` per id before Task 5.

  Each behaviour is unit-tested with the engine's test helper (no network).
- The CLI handles `RoomListLoaded` and `Avatar` by ignoring them (exhaustive match kept). Its existing tests pass.
- Gate and the ignored run (`-p twicecat-core`) pass. Commit: `phase2.2: additive core api for room list queries, previews and avatars`.

### Task 3: Room view models in the core: flags, highlights, avatar ID, preview, loaded signal (roadmap step 3)
Language: rust
Files: crates/core/src/room_list.rs, crates/core/src/engine.rs, crates/core/tests/common/mod.rs, crates/core/tests/room_list.rs, PROGRESS.md
Depends on: Task 2
Acceptance criteria:
- `summarize` fills the Q6 fields:
  - `is_dm` from `is_dm()`, `is_space` from `is_space()`, `is_invite` from `state() == RoomState::Invited`;
  - `highlights` per Q9, `marked_unread` from `is_marked_unread()`;
  - `avatar` per Q11 (room avatar, else the single DM hero's avatar; only valid MXC URIs);
  - `last_message` per Q10.

  Pure helpers carry unit tests: `highlights_of(mentions, server)`, `preview_body(raw)` (first line, control and bidi characters stripped, 120-char cap on a char boundary, with a multi-byte case), and `pick_avatar(room_avatar, is_dm, hero_avatars)`.
- The forwarder registers each newly seen room ID once with `latest_events().listen_to_room` (registration set unit-tested through a helper), so latest-event updates arrive as `Set` diffs.
- A second task (or `select!` arm) forwards `RoomList::loading_state()`: each `Loaded { maximum_number_of_rooms }` is emitted as `CoreEvent::RoomListLoaded { total_rooms }`, and `NotLoaded` emits nothing.
- Test helpers (`tests/common`) gain `create_dm(n, with)` and `create_space(n, name)` (plain SDK; never print bodies or tokens).
- Integration tests, each with the verbatim ignore reason:
  - **preview**: alice's core is syncing; the helper alice sends a unique marker text into a new room. Within 30 s, alice's applied list shows that room with `last_message.body == marker` and `is_own == true`.
  - **dm and space flags**: the helper creates a DM alice↔bob and a space. Alice's list shows the DM with `is_dm == true` and the space with `is_space == true`.
  - **loaded**: `RoomListLoaded { total_rooms: Some(n) }` arrives within 30 s of `SyncState(Running)`.
- Gate and the ignored run pass. Commit: `phase2.3: room view models with flags, highlights, avatar id and preview`.

### Task 4: Room list query (tabs, search) and paging in the core (roadmap step 5, core side)
Language: rust
Files: crates/core/src/room_list.rs, crates/core/src/engine.rs, crates/core/tests/room_list.rs, PROGRESS.md
Depends on: Task 3
Acceptance criteria:
- The engine keeps the latest `RoomListQuery` in a `tokio::sync::watch` channel. A query set before login applies when the list starts.
  - The forwarder owns the controller and, on each change, calls `set_filter(build_filter(&query))` then `reset_to_one_page()`.
  - `LoadMoreRooms` reaches the forwarder through a small channel and calls `add_one_page()`. With no active sync it's a no-op.
- `build_filter` is built from a pure `filter_spec(&RoomListQuery) -> Vec<FilterPart>`. Unit tests check the Q7 table for each tab, with and without search, and that whitespace-only search adds no name filter.
- Integration tests (verbatim ignore reason; unique room names per run):
  - **search**: two rooms `twicecat-a-<rand>` and `twicecat-b-<rand>`. After `SetRoomListQuery { search: "twicecat-a-<rand>" }` the next applied `Reset` plus diffs contain only the first.
  - **tabs**: with a DM and a space from the Task 3 helpers, `People` shows the DM and not the space, `Rooms` shows neither, and `Spaces` shows only the space.
  - **paging**: with `room_list_page_size = 2` in the test `CoreConfig` and at least 3 rooms, the list holds 2 rooms, then 3 or more after `LoadMoreRooms`.
- Gate and the ignored run pass. Commit: `phase2.4: room list tabs, search and paging via sdk filters`.

### Task 5: Avatar thumbnails through the SDK media API (roadmap step 6, core side)
Language: rust
Files: crates/core/src/avatars.rs, crates/core/src/lib.rs, crates/core/src/engine.rs, crates/core/tests/avatars.rs, crates/core/tests/fixtures/avatar.png, PROGRESS.md
Depends on: Task 4
Acceptance criteria:
- `FetchAvatars(ids)` works per Q11:
  - deduplicates against in-flight and already-answered IDs (cleared on logout);
  - runs at most 4 fetches at once;
  - uses a 96×96 `Thumbnail` with `use_cache = true` and falls back to `File` on error;
  - caps the size at 1 MiB;
  - emits exactly one `Avatar { id, image }` per requested ID.

  `avatars.rs` is a private module (not in the `check-core-api.sh` allow-list).
- Tasks live in a `JoinSet` on `Active`, aborted in `stop_tasks` before the store is released. A unit test checks the dedupe, cap and invalid-MXC logic with a fake fetcher (trait or closure; no network). An invalid MXC (`"not-mxc"`) gives `image: None` without a request.
- No file is written outside the SDK store. After a fetch, a test scans the tempdir for the fixture's bytes and finds them only inside the session's SQLite files, or not at all (encrypted store).
- `tests/fixtures/avatar.png` is a small valid PNG (64×64 or larger) committed to the repo, with its origin noted in PROGRESS.md.
- Integration test (verbatim ignore reason): the helper alice uploads the fixture as a new room's avatar (`upload_avatar`). Alice's core list shows `avatar: Some(id)`, and `FetchAvatars([id])` gives `Avatar { id, image: Some(bytes) }` with non-empty bytes within 30 s. A second `FetchAvatars([id])` gives no second event.
- Gate and the ignored run pass. Commit: `phase2.5: on-demand avatar thumbnails via sdk media cache`.

### Task 6: App library split and the core bridge (roadmap step 1)
Language: rust
Files: crates/app/src/lib.rs, crates/app/src/main.rs, crates/app/src/bridge.rs, crates/app/src/state.rs, crates/app/tests/bridge.rs, .github/workflows/ci.yml, PROGRESS.md
Depends on: Task 5
Acceptance criteria:
- The `twicecat-app` crate has a lib target plus the `twicecat` bin. `main.rs` only:
  - parses `--data-dir`, `--homeserver`, `--ephemeral`, `--help` (same meaning as the CLI; `--help` carries the Q20 note);
  - calls `logging::init` (Q19);
  - runs `iced::application(App::boot, App::update, App::view)` with `.title("Twicecat")`, `.subscription`, `.theme`, `.exit_on_close_request(false)`.

  The existing title test is kept.
- `bridge.rs` follows Q3/Q4:
  - `Bridge::start(config, secrets) -> (Bridge, EventsSlot)` builds the runtime and starts `Core`. `Bridge::send(&self, Command) -> Option<Task<Message>>` does `try_send` with a send-task fallback when the channel is full. `Bridge` shuts the runtime down with `shutdown_timeout(5 s)` on drop.
  - `events_subscription(slot) -> Subscription<Message>` maps `BridgeEvent::Core(e)` to `Message::Core(Box<CoreEvent>)` and `Closed` to `Message::CoreClosed`.
- `App` holds a `CommandSink` (the `Bridge` in production; a plain `mpsc::Sender<Command>` in tests) so that `update` is testable with no runtime. Boot sends `RestoreSession` and returns `system::theme().map(Message::ThemeMode)`.
- Unit tests:
  - the slot's stream yields the receiver's events in order, then `Closed`, and a second build yields an empty stream;
  - `update(CloseRequested)` sends `Shutdown` once and moves to `ShuttingDown`;
  - `update(CoreClosed)` while `ShuttingDown` returns the exit path, tested via a state flag, since `Task`s are opaque;
  - with a full capacity-1 sink, `update` doesn't block (returns within 50 ms) and still queues the command through the fallback.
- `tests/bridge.rs` (verbatim ignore reason) starts a real `Bridge` with `--ephemeral`-style settings against the local homeserver and drives `update` headlessly:
  - it feeds `Message::LoginSubmitted` with alice's credentials and pumps events from the slot's stream into `update`;
  - the state reaches `Screen::Main` and holds at least one room within 60 s;
  - it sends `Shutdown`, and `CoreClosed` arrives within 10 s.
- The CI `integration` job command becomes `cargo test -p twicecat-core -p twicecat-app --locked -- --ignored --skip keyring` (Q16). actionlint is clean.
- Gate and the ignored run (both packages) pass; `scripts/check-app-deps.sh` exits 0. Commit: `phase2.6: app library split and core bridge via subscription`.

### Task 7: Room list and timeline state from diffs, and the 2,000-room timing test (roadmap step 2)
Language: rust
Files: crates/app/src/room_list.rs, crates/app/src/state.rs, crates/core/src/api/diff.rs (only if `apply_owned` is needed), PROGRESS.md
Depends on: Task 6
Acceptance criteria:
- `Message::Core(RoomListChanged(diffs))` applies each diff in order to `state.rooms: Vec<RoomSummary>`. The whole list is never rebuilt except on `Reset` or `Clear`.
  - `RoomListLoaded` stores `total_rooms`.
  - The selected room is kept by `RoomId`. It is cleared (and `selected == None`) when a diff removes it, and the right pane follows.
  - `TimelineDiff` for the selected room applies to `state.timeline: Vec<TimelineItem>`; diffs for any other room ID are ignored.
- **Every variant through `update`:** 11 unit tests, one per `ListDiff` variant (Append, Clear, PushFront, PushBack, PopFront, PopBack, Insert, Set, Remove, Truncate, Reset). Each starts from a known 3-room state, calls `update` with a one-diff `RoomListChanged`, and compares the room IDs and names to a hand-written expected list. Plus a multi-diff batch test and an out-of-range test (ignored, no panic).
- **Timing (Q17):** test `room_list_batches_under_16ms` times these batch shapes on deterministic synthetic rooms (each with name, avatar ID and a 100-char preview):
  1. `Reset` of 2,000;
  2. 4 × `Append` of 500 from empty;
  3. on a 2,000-room list, a mixed batch of 100 `Set`, 50 `Remove`+`Insert` pairs (reorders) and 10 `PushFront`;
  4. `Clear` then `Reset` of 2,000.

  Median of 15 runs (after a warm-up) under 16 ms for each, measured with `std::time::Instant` around `update` only. Timings are printed with `--nocapture` and recorded in PROGRESS.md (debug profile, local machine).
- If `ListDiff::apply_owned` is added, it gets 11 variant tests mirroring `apply`'s, and the API stays additive.
- Gate passes. Commit: `phase2.7: diff-driven room list and timeline state with timing test`.

### Task 8: Light and dark theme following the system (roadmap step 7)
Language: rust
Files: crates/app/src/theme.rs, crates/app/src/lib.rs, PROGRESS.md
Depends on: Task 7
Acceptance criteria:
- `theme.rs` defines `LIGHT` and `DARK` `Palette` constants, `fn theme_for(mode: Mode) -> Theme` (`Dark` gives "Twicecat Dark"; `Light` and `None` give "Twicecat Light"), and the semantic colours used by later tasks: badge, highlight badge, muted text and selection.
- `App::theme` returns `Some(theme_for(state.mode))`. `Message::ThemeMode(mode)` comes from boot's `system::theme()` and from `system::theme_changes()` in the subscription batch.
- Unit tests:
  - `theme_for` names and `extended_palette().is_dark` for all 3 modes;
  - `update(ThemeMode(Dark))` changes `App::theme`'s result;
  - WCAG contrast ratio of at least 4.5 for text on background, and for badge text on the badge colour, in both palettes (a pure `contrast(a, b)` helper).
- Human checklist gains:
  - the app follows the OS light/dark setting at start, and when switched while running, on Linux (GNOME/KDE portal), macOS and Windows;
  - both palettes are readable.
- Gate passes. Commit: `phase2.8: light and dark palettes following the system theme`.

### Task 9: Screens: login with OIDC, loading/sync, two-pane main layout (roadmap step 4)
Language: rust
Files: crates/app/src/screens/mod.rs, crates/app/src/screens/login.rs, crates/app/src/screens/loading.rs, crates/app/src/screens/main_view.rs, crates/app/src/state.rs, PROGRESS.md
Depends on: Task 8
Acceptance criteria:
- `Screen` state machine (`Starting`, `Login`, `WaitingForBrowser`, `Loading`, `Main`, `ShuttingDown`), with transitions in `update` only and unit-tested:
  - boot gives `Starting`; `NoSession` gives `Login`;
  - `LoggedIn` gives `Loading`;
  - `Loading` becomes `Main` on `RoomListLoaded` or a non-empty room list;
  - `LoginFailed(e)` gives `Login` with a non-secret error line, and the password field is cleared;
  - `LoggedOut` and `SessionExpired` give `Login` with a notice;
  - `OAuthUrl` gives `WaitingForBrowser`; Cancel sends `CancelOAuth`.
- Login form fields: server (optional; the hint says it's taken from the MXID), username, and password (`secure(true)`, held as `Password` per Q19). Enter or **Log in** sends `LoginPassword`. While a login is in flight, buttons are disabled and a second submit sends nothing (unit test).
- **Sign in with browser** sends `LoginOAuth { server }`, and is disabled when server is empty. On `OAuthUrl`:
  - `check_browser_url` (pure; unit tests for https OK, loopback http OK, other http refused, userinfo refused, `javascript:` refused) runs first;
  - then a `Task::perform` calls `open::that_detached`;
  - on failure, **Copy link** appears (`clipboard::write`).

  `update` never calls `open` directly (checked by a grep test, or by keeping the call in a function only `Task`s reach).
- Loading screen: shows the latest `SyncState` (Running / Offline / Reconnecting { attempt }) and the loaded room count.
- Main layout: a left room-list pane (filled by Task 10) of fixed width, and a right pane per Q15:
  - the selected room's name;
  - the last 50 `Message` items as plain `sender: body` text;
  - a single-line composer holding `MessageText`. Enter sends `SendMessage` and clears the field; it's disabled with no selected room.
  - `SendFailed` shows a non-secret error.

  A status bar shows the sync state, and a **Log out** button sends `Logout`.
- Redaction unit tests: `format!("{:?}")` of `Message::PasswordChanged`, of `Message::ComposerChanged`, and of the app state after typing `hunter2-x` / `secret-body-x` contains neither.
- Human checklist gains:
  - password login through the window (local Synapse and the real account), and restart restores without the login screen;
  - the OIDC button opens the default browser on each OS and completes on matrix.org; Cancel works;
  - Copy link works when no browser is available;
  - the loading screen appears during the initial sync;
  - send a message from the composer and see it from another client.
- Gate passes; `scripts/check-app-deps.sh` exits 0. Commit: `phase2.9: login, loading and two-pane main screens`.

### Task 10: Room list pane: tabs, search, windowed rows, badges, paging (roadmap step 5, UI side)
Language: rust
Files: crates/app/src/screens/room_list_pane.rs, crates/app/src/room_list.rs, crates/app/src/screens/main_view.rs, crates/app/src/state.rs, PROGRESS.md
Depends on: Task 9
Acceptance criteria:
- Tabs row (All, DMs, Rooms, Spaces) and a search `text_input`. Changing either sends exactly one `SetRoomListQuery(RoomListQuery { tab, search })`, and the state keeps the query (unit tests). An identical query isn't re-sent.
- The list is a `scrollable` with fixed `ROW_HEIGHT` rows, windowed per Q12:
  - pure `visible_range(offset_y, viewport_h, row_h, len, overscan) -> Range<usize>`, unit-tested at top, middle and end, with an empty list, a viewport taller than the list, and `len` changing under a stale offset;
  - the rendered window plus top and bottom spacers keep the total content height `len * ROW_HEIGHT` (unit test on the computed heights).
- Each row shows the avatar or initials slot (filled by Task 11), the name, the preview (`sender_name` or "You": body, truncated), an "Invite" tag for invites, and the badge from the pure Q9 `badge_for(&RoomSummary) -> Badge`. The badge function has a unit test per rule and per precedence.
- Clicking a row sends `SelectRoom(id)` and marks it selected (unit test). The selection survives diffs that move the room.
- Paging per Q8: pure `should_load_more(visible_end, len, total)` is unit-tested, and `update` sends `LoadMoreRooms` at most once per loaded length (unit test with a scripted sequence of scrolls and appends).
- Empty states: "No rooms" per tab, and "No match" with a search.
- Human checklist gains:
  - the list updates live (a message from another client moves the room up, the unread badge grows, the preview changes, a mention shows the highlight badge);
  - tabs and search behave;
  - scrolling a 2,000+-room account stays smooth;
  - the UI stays responsive while typing in search during the initial sync of a large account.
- Gate passes. Commit: `phase2.10: room list pane with tabs, search, badges and windowed rows`.

### Task 11: Avatars in the UI with initials placeholder (roadmap step 6, UI side); Phase 2 wrap-up
Language: rust
Files: crates/app/src/avatars.rs, crates/app/src/screens/room_list_pane.rs, crates/app/src/state.rs, PROGRESS.md
Depends on: Task 10
Acceptance criteria:
- After each room-list change, scroll, or query change, `update` collects the `avatar` IDs of the rows in `visible_range` that aren't yet requested, and sends them in one `FetchAvatars` (no command when the set is empty). Unit tests check: a `Reset` of 100 rooms with a 20-row viewport requests exactly those rows' avatar IDs; a repeat requests nothing; scrolling requests only the new rows.
- `Avatar { id, image: Some(bytes) }` stores `image::Handle::from_bytes(bytes)` in `HashMap<AvatarId, AvatarState>`, and `None` stores `Unavailable` (no retry). The map is cleared on `LoggedOut` and `SessionExpired` (unit tests).
- Placeholder:
  - pure `initials(name) -> String`: up to 2 uppercase letters from the first two words, skipping a leading `#`, `@` or `!`, Unicode-safe on char boundaries, with `"?"` for empty input. Unit tests cover plain, single-word, `#room:server`, `@user:server`, non-Latin and emoji names.
  - `placeholder_color(room_id)` is deterministic and drawn from a fixed palette list (unit test: same ID gives the same colour).
  - The placeholder shows until the image arrives, and stays if it's unavailable.
- The Task 7 timing test still passes with avatar bookkeeping in `update` (it's included in the measurement).
- PROGRESS.md marks the Phase 2 agent work done. The human checkpoint stays open. The open-issues list includes: iced text-input buffers aren't zeroized (Q19), decoding untrusted thumbnails relies on the `image` crate (bounded by the 1 MiB cap), and the CLI and app share one data dir (Q20).
- Human checklist gains: avatars render for rooms and DMs; initials show for rooms without an avatar; no flicker while scrolling.
- Gate and the ignored run (both packages) pass. Commit: `phase2.11: avatar thumbnails and initials placeholders in the room list`.

### Phase 2 human checkpoint (collected in PROGRESS.md)
- [x] Open questions Q1–Q22 resolved under the standing pre-authorization (2026-10-01); revisit at this checkpoint.
- [ ] CI green on Linux, Windows and macOS with the new dependencies (image codecs, open), and the `integration` job green with the app tests.
- [ ] On each OS: password login through the window, then restart restores without the login screen.
- [ ] The OIDC button opens the default browser on each OS; login completes on matrix.org; Cancel works; Copy link works when no browser opens.
- [ ] Watch the room list update live: ordering, unread and highlight badges, previews, invites tagged.
- [ ] Tabs (All / DMs / Rooms / Spaces) and search filter correctly on the real account.
- [ ] Avatars render, initials placeholders show for rooms without one, and nothing flickers while scrolling.
- [ ] The theme follows the system light/dark setting at start and on a live switch; both palettes are readable.
- [ ] The UI stays responsive (scrolling, typing in search) during the initial sync of a large account.
- [ ] Closing the window exits within a few seconds, with no leftover process.
- [ ] Composer: a message sent from the right pane arrives in another client.
- [ ] Approve the additive API (Q6) as the Phase 3 baseline.
