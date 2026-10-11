# Phase 3 Timeline and Composer Plan
Status: APPROVED

## Goal
Replace the Phase 2 right pane (the last 50 messages as one-line plain text, plus a `text_input`) with a real room view.

The timeline pane renders every kind of item the SDK `Timeline` produces:
- messages, grouped by sender;
- day separators, the read marker and the start of the room;
- state events (joins, leaves, invites, name, topic and avatar changes) in a muted style;
- placeholders for redacted, undecryptable and media events.

It is virtualized. Only the rows around the viewport are built and laid out, using measured and estimated row heights, and it stays smooth on a 10,000-item timeline. Scrolling near the top paginates backwards (20 events at a time), and the scroll position stays put while items are prepended.

Formatted bodies are handled in the core. The core parses the HTML `formatted_body` with the HTML parser already in the SDK tree (`ruma::html`, built on html5ever 0.39), maps an allow-list of tags into an SDK-free `RichText` model, and falls back to the plain `body`. The app maps `RichText` to Iced `rich_text` spans after the `text_safety` sanitiser. Links open through an allow-list. `matrix.to` links and mentions render as pills.

The composer is an Iced `text_editor`:
- Enter sends and Shift+Enter adds a line;
- markdown becomes HTML on send (ruma's pulldown-cmark wrapper, behind the SDK's `markdown` feature);
- mention autocomplete draws on room members, and emoji autocomplete handles `:smile:`.

Message actions are reply (with a quote), edit, redact, react (with an emoji picker) and copy. Read receipts and typing notifications are both sent and displayed. Local echoes from the SDK `SendQueue` show pending and failed states, with Retry and Discard.

The core API grows additively, under the Phase 2 Q6 convention, and Q7 records the two semantic adjustments. The agent proves this work with:
- fixture tests for the HTML mapper and the markdown sender;
- homeserver round-trip tests for reply, edit, redact and react;
- a 10,000-item 16 ms batch timing test;
- a headless layout test showing that only the visible rows of a 10,000-item timeline are laid out.

Rendering, scrolling feel, IME and a day of real chat stay on the human checkpoint.

### Facts verified while planning (2026-10-02)
Registry root: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Each implementing task re-confirms the APIs it uses (AGENTS.md rule 2) and cites file:line in PROGRESS.md.

Two probes were compiled in a scratch copy of the workspace in the session scratchpad, not in the repo:
- a core module calling every SDK API below (with the `markdown` feature on), which passed `cargo check`;
- an app module calling every iced, `emojis` and `chrono` API below, including a headless tiny-skia layout of a 30-row scrollable, which passed `cargo test`.

**Timeline (matrix-sdk-ui 0.19.1, `src/timeline/`)**
- `Timeline` methods:
  - `subscribe` mod.rs:332, `send` :363, `send_reply(RoomMessageEventContentWithoutRelation, OwnedEventId)` :451;
  - `edit(&TimelineEventItemId, EditedContent)` :556, `toggle_reaction(&TimelineEventItemId, &str) -> Result<bool>` :643, `redact(&TimelineEventItemId, Option<&str>)` :724;
  - `send_single_receipt(ReceiptType, OwnedEventId)` :849, `mark_as_read(ReceiptType)` :984, `items()` :1022.
- Pagination: `paginate_backwards(u16) -> Result<bool /* hit start */>` pagination.rs:30, and `live_back_pagination_status() -> Option<(PaginationStatus, Stream)>` :124. `matrix_sdk::event_cache::PaginationStatus { Idle { hit_timeline_start }, Paginating }` lives in matrix-sdk event_cache/caches/pagination.rs:379.
- `RoomExt::timeline()` (traits.rs:76-82) already uses `track_read_marker_and_receipts(AllEvents)`. The default settings use `DateDividerMode::Daily` (controller/mod.rs:261-270), and dividers fall on local-time days (date_dividers.rs:20, chrono `Local`).
- `VirtualTimelineItem::{DateDivider(ms) :25, ReadMarker :28, TimelineStart :32}` (virtual_item.rs:19). `TimelineItem::unique_id()` (item.rs:86) is the stable ID that Phase 2 already exposes as `TimelineItem::id`.
- `EventTimelineItem` methods (event_item/mod.rs):
  - `send_state` :224, `identifier() -> TimelineEventItemId` :254, `event_id` :279, `sender_profile() -> &TimelineDetails<Profile>` :292;
  - `content` :313, `read_receipts() -> &IndexMap<OwnedUserId, Receipt>` :328, `is_own` :347, `is_editable` :355;
  - `is_highlighted` :391, `can_be_replied_to` :461, `original_json` :481, `latest_edit_json` :489, `latest_json` :498, `local_echo_send_handle() -> Option<SendHandle>` :642.
  - `Profile { display_name, display_name_ambiguous, .. }` :728.
- Content:
  - `TimelineItemContent::{MsgLike, MembershipChange, ProfileChange, OtherState, FailedToParse*, CallInvite, RtcNotification}` content/mod.rs:97.
  - `MsgLikeContent { kind, reactions, in_reply_to, thread_root, thread_summary }` msg_like.rs:68, with `MsgLikeKind::{Message, Sticker, Poll, Redacted, UnableToDecrypt, Other, LiveLocation}` :27.
  - `Message::{msgtype :76, body :83, is_edited :89}`.
  - `InReplyToDetails { event_id, event: TimelineDetails<Box<EmbeddedEvent>> }` reply.rs:34.
  - `ReactionsByKeyBySender` derefs to `IndexMap<String, IndexMap<OwnedUserId, ReactionInfo>>` (event_item/mod.rs:842).
  - `MembershipChange` has 17 variants (content/mod.rs:691-741). `MemberProfileChange::displayname_change` is at :762, `avatar_url_change` at :767. `AnyOtherStateEventContentChange::{RoomName, RoomTopic, RoomAvatar, RoomCreate, RoomEncryption, ..}` is at :789-843.
- **The SDK already sanitizes every `formatted_body`** in `Compat` mode and strips the reply fallback (message.rs `from_event`, lib.rs:35 `DEFAULT_SANITIZER_MODE`). The ruma sanitizer *unwraps* disallowed elements and keeps their children (ruma-html sanitizer_config/clean.rs:29-45, `NodeAction::Ignore`). So the text inside a `<script>` survives as plain text in `Message::msgtype()`. The raw event JSON is still available through `latest_json()` (`Raw::get_field`, ruma-common serde/raw.rs:106) for remote events; local echoes have none.
- **Send queue** (matrix-sdk 0.19.1 send_queue/mod.rs):
  - **after any send error the room's queue disables itself** (:1018-1019);
  - errors are recoverable only for transient or network HTTP failures (:999-1016), and unrecoverable requests are "wedged" and block later ones (module doc);
  - `SendHandle::unwedge` is at :2986 and `SendHandle::abort` at :2838;
  - `RoomSendQueue::set_enabled(bool)` :1232 (sync) and `SendQueue::set_enabled(bool).await` :286.
  - `EventSendState::SendingFailed { error, is_recoverable }` is at timeline event_item/local.rs:69. Nothing in matrix-sdk-ui re-enables the queue on its own.
- **Room** (matrix-sdk room/mod.rs):
  - `typing_notice(bool)` :2104, throttled by the SDK: 4 s timeout, resent after 3 s (:227-228);
  - `subscribe_to_typing_notifications() -> (EventHandlerDropGuard, broadcast::Receiver<Vec<OwnedUserId>>)` :661, which filters out the own user;
  - `sync_members` :1169, `members_no_sync(RoomMemberships)` :1246 (`RoomMemberships::JOIN`, base room/members.rs:528), and `RoomMember::{user_id, display_name, name}`;
  - `EditedContent::RoomMessage` is at room/edit.rs:39.
- **Sliding sync:** `RoomListService::set_room_subscriptions(&[&RoomId])` room_list_service/mod.rs:485. The typing extension is on, and the receipts extension covers `AllSubscribed` (:169-176). The service is reached through `SyncService::room_list_service()` (sync_service.rs:612). Phase 2 doesn't subscribe the selected room today.
- **HTML parsing in the tree:**
  - matrix-sdk-ui turns on ruma's `html` feature (matrix-sdk-ui Cargo.toml:185), so `matrix_sdk::ruma::html` (ruma lib.rs:156-158) gives `Html::parse` (ruma-html html.rs:37), `children()`, `NodeData::{Element, Text, ..}` :297 and `ElementData { name, attrs: RefCell<BTreeSet<Attribute>> }` :314, on html5ever 0.39.0 (already in Cargo.lock);
  - ruma-html's `matrix` feature (`to_matrix`) is **off**, so mentions are parsed from the `href`;
  - `MatrixToUri::parse` and `MatrixUri::parse` are at ruma-common identifiers/matrix_uri.rs:262/:283/:447, with `MatrixId::{Room, RoomAlias, User, Event}` :24.
- **Markdown:**
  - matrix-sdk has the feature `markdown = ["ruma/markdown"]` (Cargo.toml:120), and ruma-events then depends on `pulldown-cmark ^0.13.0` with `default-features = false, features = ["html"]`;
  - `RoomMessageEventContentWithoutRelation::text_markdown` (without_relation.rs:61, `cfg(feature = "markdown")`) and `add_mentions` (:274), plus `Mentions::with_user_ids` (ruma-events lib.rs:296);
  - ruma's `parse_markdown` (room/message.rs:815) enables tables and strikethrough and turns soft breaks into hard breaks. **It leaves `formatted` unset when the text has no markdown.**
- **Lock and licence effects, measured in the probe** (`git diff`-style comparison of Cargo.lock):
  - the `markdown` feature adds exactly 3 packages: pulldown-cmark 0.13.4 (MIT), pulldown-cmark-escape 0.11.0 (MIT) and unicase 2.9.0 (MIT OR Apache-2.0);
  - `emojis` 0.9.0 (license `(MIT OR Apache-2.0) AND Unicode-3.0`, a dependency of matrix-sdk-ui at event_item/mod.rs:707) and `chrono` 0.4.45 (with `clock` already on in the tree) are already locked, so they add only edges;
  - iced's `advanced` feature (iced Cargo.toml:49) adds no package.
  - `cargo deny check` gives `bans ok, licenses ok, sources ok`.
- **Pre-existing cargo-deny failure:** on today's `main`, unchanged, `cargo deny check advisories` **fails** with RUSTSEC-2026-0319 ("anymap2 is unmaintained", via matrix-sdk 0.19.1, no safe upgrade). This isn't caused by this plan. See Q3.
- **emojis 0.9.0:** `get_by_shortcode` lib.rs:740, `get` :723, `iter` :687, `Emoji::{as_str :282, name :308, shortcode :490, shortcodes :515, group :365}`, `Group` :191.
- **iced 0.14 widgets** (iced_widget 0.14.2):
  - `text_editor` helpers.rs:1491, with `.id` :166, `.on_action` :216 and `.key_binding(Fn(KeyPress) -> Option<Binding<M>>)` text_editor.rs:313;
  - `Binding::{Enter, Custom(M), ..}` :1119, `KeyPress { key, modified_key, modifiers, text, status, .. }` :1154 and `Binding::from_key_press` :1177;
  - `Content::{new :400, with_text :405, perform :412, cursor :426, line :436, text :455}`. **`Content`'s `Debug` prints the editor buffer (:508)**, and `Action::Edit(Edit::Paste(Arc<String>))` (iced_core text/editor.rs:103) carries pasted text in its `Debug`.
- **iced 0.14 text and scrolling:**
  - `rich_text` helpers.rs:1194 and `span` :1238. `Span::{font :435, color :447, link :459, background :471, border :500, padding :532, underline :538, strikethrough :544}` is in iced_core text.rs, and `Rich::on_link_click` in text/rich.rs:140. Italic is `Font { style: font::Style::Italic, .. }`, and `Font::MONOSPACE` exists.
  - `sensor` helpers.rs:1014, with `on_show(Fn(Size))` :63, `on_resize(Fn(Size))` :71, `on_hide` :80, `key` :88 and `anticipate` :133.
  - `scrollable`: `.id` :144, `.on_scroll` :164 and `.anchor_bottom()` :175. **With the bottom anchor the stored offset is the distance from the bottom** (scrollable.rs:1713-1724). Content added at the top therefore doesn't move the view; content added or resized below the viewport does.
  - Operations: `iced::widget::operation::{scroll_to :31, scroll_by :42, snap_to_end :23, focus}` (iced_runtime widget/operation.rs; re-exported by iced lib.rs:634-637).
  - Also available: `mouse_area` (`on_enter`), `tooltip`, `float` :2084, `stack` :561 and `iced::keyboard::listen`.
- **Headless layout:**
  - `iced::advanced` only exists with the `advanced` feature (iced lib.rs:524-525);
  - `advanced::renderer::Headless::new(Font, Pixels, Some("tiny-skia"))` (iced_core renderer.rs:121, tiny-skia impl iced_tiny_skia lib.rs:434, fallback renderer.rs:680), `advanced::widget::Tree::new` and `Widget::layout(&mut Tree, &Renderer, &Limits) -> layout::Node` (iced_core widget.rs:56) with `Node::children` (layout/node.rs:48);
  - the probe laid out a 30-row column headlessly on CPU in 0.32 s.
- **Current repo state:**
  - `TimelineItemKind` is `Message { sender, body, is_own, timestamp_ms, sent } | Other`; text messages only, everything else is `Other` (core timeline.rs:36-56).
  - `TimelineItem { id, kind }` is built literally in core and app tests.
  - The engine handles commands one at a time (engine.rs:391-416), so a network await in `handle` blocks every later command.
  - `Selected { room_id, timeline, task }` engine.rs:105.
  - `SendMessage` sends `text_plain` (engine.rs:668-684).
  - The app keeps `timeline: Vec<TimelineItem>` and `composer: MessageText` (state.rs:167/:188), and `main_view::room_pane` renders the Phase 2 list (main_view.rs:83-125).
  - `text_safety::one_line*` is the only sanitiser and is one-line by design.
- **No rule-8 blocker.** Every API the roadmap names exists in the pinned versions: `Timeline` actions, `paginate_backwards`, the send queue, the text editor and rich text. The roadmap's suggested `html5ever` is used through `ruma::html` and `pulldown-cmark` through ruma's wrapper.

### Decisions (RECOMMENDED; awaiting the human's approval through the controller)
1. **Q1: precondition (AGENTS.md rule 1). DECIDED by the human, 2026-10-02.**
   - Phase 2 is signed off: PR #3 is merged and CI is green on all 3 OSes.
   - The additive core API (Phase 2 Q6) is approved as the Phase 3 baseline.
   - A Unicode general-category crate is **not** approved. `text_safety` keeps its hand-written ranges, and no task adds such a dependency.

   Task 1 records all three in PROGRESS.md:
   - it ticks the Phase 2 CI item, the "Approve the additive API (Q6)" item, and the Unicode-crate item (marked "rejected");
   - it adds the note "Phase 2 signed off by the human on 2026-10-02 (merge of PR #3)";
   - it leaves the other unticked Phase 2 items as known open checks.

   Phase 2 open issues that Phase 3 depends on stay as they are, and the plan doesn't change them:
   - the hand-written `text_safety` ranges;
   - stale same-room diffs after a quick reselect (see Q8).
2. **Q2: new dependencies (rule 3; each needs approval).** RECOMMENDED:
   - `crates/core`: matrix-sdk gains the `markdown` feature: `features = ["bundled-sqlite", "local-server", "markdown"]`, pin `=0.19.1` unchanged. It adds pulldown-cmark 0.13.4, pulldown-cmark-escape 0.11.0 and unicase 2.9.0 (all MIT or MIT/Apache).
   - `crates/app`:
     - `emojis = "0.9"` (0.9.0, already locked);
     - `chrono = { version = "0.4", default-features = false, features = ["clock"] }` (0.4.45, already locked);
     - a `[dev-dependencies]` entry `iced = { version = "=0.14.0", features = ["image-without-codecs", "advanced"] }`. It's only for the headless layout test (Q15), adds no package, and leaves the normal build's features untouched.
   - Caret ranges, locked by Cargo.lock, like Phase 1 and 2.
   - Rejected:
     - `scraper` (selectors, cssparser and ego-tree, none of them in the tree);
     - a direct `html5ever` in the app (needs a DOM sink crate, `markup5ever_rcdom`, not in the tree, and would put untrusted-HTML parsing in the app);
     - a direct `pulldown-cmark` (duplicates ruma's Matrix-specific rules, such as plain text staying plain);
     - iced's `markdown` and `highlighter` features (they render markdown, not Matrix HTML);
     - `linkify` and `url` in the app (the core already has `url`; Q19);
     - `time` and `jiff` (chrono is already in the tree);
     - `gemoji` and `emojic` (`emojis` is already in the tree).
3. **Q3: deny.toml and the pre-existing advisory.** RECOMMENDED: Task 1 adds `{ id = "RUSTSEC-2026-0319", reason = "anymap2 unmaintained, transitive via matrix-sdk 0.19.1; no fix without changing the pin" }` to `[advisories] ignore`, and changes nothing else. This needs explicit human approval: the failure is already on `main` and blocks the gate (rule 4) for every task. Rejected: waiting for an SDK release (that would change the pin, rule 3).
4. **Q4: where HTML becomes rich text.** RECOMMENDED: in the core, in a private module `crates/core/src/rich_text.rs` on `matrix_sdk::ruma::html`. It produces the SDK-free `api::RichText` (Q5).
   - Source: the raw `content.formatted_body`, or for an edit `content["m.new_content"].formatted_body`, read from `latest_json()` when `format == "org.matrix.custom.html"`. This lets `script` and `style` be dropped **with their contents**, which the SDK's own sanitize pass can't do (see Facts).
   - Fallbacks:
     - local echoes and events with no raw JSON use the SDK-sanitized `formatted` body;
     - with no formatted body, a non-HTML format, or a body whose mapped text is blank, the plain `body` is used, split into paragraphs on line breaks and linkified (Q19).
   - Parsing runs on the core runtime, never on the UI thread. The app only maps `RichText` to spans.
   - Rejected:
     - parsing in the app (untrusted parser on the UI thread, plus a new dependency);
     - trusting the SDK-sanitized string alone (script text would show);
     - passing HTML strings through the API.
5. **Q5: the allow-list and `RichText` model.** RECOMMENDED:
   - `RichText { blocks: Vec<Block> }`, with:
     - `Block::{Paragraph(Vec<Inline>), Heading { level, inlines }, Quote(Vec<Block>), List { ordered, start, items: Vec<Vec<Block>> }, Code { text }, Rule}`;
     - `Inline::{Text { text, style: TextStyle }, Link { text, url, style }, Mention { target: MentionTarget, label }, LineBreak, Spoiler(Vec<Inline>)}`;
     - `TextStyle { bold, italic, strike, underline, code }` (`Copy + Default`);
     - `MentionTarget::{User(UserId), Room(String)}`.
   - Mapping:
     - `b`/`strong` gives bold, `i`/`em` italic, `del`/`s`/`strike` strike, `u` underline, and inline `code` code;
     - `pre` (with an optional inner `code`) gives `Block::Code` with whitespace kept;
     - `a[href]` gives a `Link` (Q19) or a `Mention`;
     - `blockquote` gives `Quote`, `ul`/`ol[start]` with `li` give `List`, `h1`-`h6` give `Heading`, `p`/`div` give paragraph breaks, `br` gives `LineBreak`, and `hr` gives `Rule`;
     - `span[data-mx-spoiler]` gives `Spoiler`;
     - `img` gives its `alt` text (or nothing), `table` rows give one paragraph per row with cells joined by two spaces, and `sup`/`sub`/`font`/`span`/`details`/`summary` are unwrapped (children kept, colours ignored).
   - **Dropped with contents:** `script`, `style`, `head`, `title`, `template`, `iframe`, `object`, `embed`, `noscript`, `svg`, `math`, `mx-reply`. Any other element is unwrapped.
   - Caps (constants, each unit-tested): nesting depth 32 (deeper content is flattened to text), 2,000 inlines, 16,384 chars of text (then an ellipsis), 1,000 list items, URLs over 2,048 bytes become plain text.
   - `Debug` of `RichText` shows only block and inline counts.
   - Rejected: keeping colours (`data-mx-color`; contrast and accessibility in both themes), and rendering `img` (media is a later phase).
6. **Q6: plain-text fallback and linkify.** RECOMMENDED: the core linkifies `https://` and `http://` tokens in plain bodies and in HTML text outside `a` with a small pure scanner: a token ends at whitespace, or at trailing `.,;:!?)` / `'"` / `>`. Each URL goes through Q19 normalization. `mailto:` isn't auto-linked. Rejected: the `linkify` crate (new dependency).
7. **Q7: additive core API.** RECOMMENDED: no existing field or variant is removed or renamed. Two meanings are adjusted and recorded here:
   - **(a)** `TimelineItemKind::Message` now covers `m.text`, `m.notice` and `m.emote`, told apart by `details.kind`, where Phase 2 only had `m.text`.
   - **(b)** `TimelineItemKind::Other` narrows to "anything not covered by the new variants". Consumers that skip `Other` (the CLI, Phase 2 tests) still behave.

   Additions (`api/types.rs`, `api/mod.rs`):
   - **Types:**
     - `RichText`, `Block`, `Inline`, `TextStyle`, `MentionTarget` (Q5);
     - `MessageKind { Text, Notice, Emote }`;
     - `MessageDetails`, with:
       - `sender_name: Option<String>`, `sender_name_ambiguous: bool`, `kind`;
       - `content: RichText` (always filled, Q4);
       - `edited`, `highlighted`;
       - `reply_to: Option<ReplyPreview>`;
       - `reactions: Vec<ReactionGroup>`;
       - `actions: ItemActions { can_reply, can_edit, can_redact, can_react, can_copy }`;
       - `send_failure: Option<SendFailure { recoverable: bool }>`.

       It derives `Default`, has a manual `Debug` that hides content, reply body and reaction keys, and is boxed in the variant.
     - `ReplyPreview { sender: UserId, sender_name: Option<String>, body: String /* one line, ≤200 chars */, state: ReplyState { Ready, Loading, Unavailable } }`, with a redacted `Debug`;
     - `ReactionGroup { key: String, count: u32, own: bool, senders: Vec<UserId> /* ≤5 */ }`;
     - `StateChange` (Q23), `PlaceholderKind { Redacted, UnableToDecrypt, Image, Video, Audio, File, Sticker, Poll, Location, Unsupported }`;
     - `PaginationState { Idle, Loading, ReachedStart }`, `TypingUser { user_id, name: Option<String> }`, `Member { user_id, display_name: Option<String> }`;
     - `MentionRef { label: String, user_id: UserId }`;
     - `OutgoingMessage { room_id: RoomId, text: MessageText /* markdown source */, mentions: Vec<MentionRef>, reply_to: Option<String /* item id */>, kind: MessageKind }`, whose `Debug` shows lengths only;
     - `ActionKind { Send, Edit, Redact, React, Retry, Discard, Paginate, Receipt, Typing, Members }`.
   - **Changes to existing types:**
     - `TimelineItemKind::Message` gains the field `details: Box<MessageDetails>`. `TimelineItemKind` gains:
       - `DateDivider { timestamp_ms: u64 }`, `ReadMarker`, `TimelineStart`;
       - `State { sender: UserId, sender_name: Option<String>, timestamp_ms: u64, change: StateChange }`;
       - `Placeholder { sender: UserId, sender_name: Option<String>, is_own: bool, timestamp_ms: u64, what: PlaceholderKind }`.
     - `TimelineItem` gains `read_by: Vec<UserId>` (others only, at most 20), and a constructor `TimelineItem::new(id, kind)` with an empty `read_by`.
     - `CoreError` gains `ItemNotFound` and `NotAllowed`.
   - **Commands:**
     - `Send(OutgoingMessage)`;
     - `Edit { item_id: String, message: OutgoingMessage }`;
     - `Redact { room_id, item_id: String, reason: Option<MessageText> }`;
     - `ToggleReaction { room_id, item_id: String, key: String }`;
     - `RetrySend { room_id, item_id: String }`, `DiscardSend { room_id, item_id: String }`;
     - `PaginateBackwards { room_id }`;
     - `MarkRead { room_id, fully_read: bool }`;
     - `SetTyping { room_id, typing: bool }`;
     - `SearchMembers { room_id, query: String }`.
   - **Events:**
     - `Pagination { room_id, state: PaginationState }`;
     - `Typing { room_id, users: Vec<TypingUser> }`;
     - `Members { room_id, query: String, members: Vec<Member> }`;
     - `ActionFailed { room_id, action: ActionKind, error: CoreError }`.
   - `SendMessage { text }` keeps its Phase 2 meaning (plain text to the selected room; the CLI uses it).

   Struct-literal call sites get `details: Box::default()` or use `TimelineItem::new`, as Phase 2 did with `RoomSummary::new`.

   Rejected:
   - restructuring `Message` into `Message(Box<MessageItem>)` (breaking, with no gain);
   - `#[non_exhaustive]` (changes the approved contract);
   - sending HTML or SDK IDs across the API.
8. **Q8: addressing items.** RECOMMENDED:
   - Actions name an item by `TimelineItem::id` (the SDK `unique_id`) plus the `room_id`.
   - The core accepts actions only for the currently selected room. A mismatch gives `ActionFailed { error: NoRoomSelected }`.
   - The core resolves the item in `timeline.items()` by `unique_id`, then uses `identifier()` (edit, redact, react), `event_id()` (reply, receipts) or `local_echo_send_handle()` (retry, discard). A missing item gives `ItemNotFound`. A disallowed action (not `is_editable`, not `can_be_replied_to`, someone else's message for edit) gives `NotAllowed`.
   - Stale diffs after a quick A→B→A reselect (Phase 2 open issue) stay tolerated: the next `Reset` corrects the list, and a stale item ID fails cleanly with `ItemNotFound`.

   Rejected: putting SDK event IDs or transaction IDs into the API.
9. **Q9: engine concurrency.** RECOMMENDED:
   - `Selected` holds `Arc<Timeline>`, a `JoinSet<()>` for action tasks, the typing forwarder (with its `EventHandlerDropGuard`) and the pagination-status forwarder. All are aborted in `Selected::stop` before the timeline is dropped, so logout and shutdown stay clean.
   - Every network action (send-reply, edit, redact, react, retry, discard, paginate, receipts, typing, member sync) runs in that `JoinSet`, so `handle` never waits on the network.
   - At most one back-pagination runs at a time: a second request while one is running is a no-op.
   - Failures emit `ActionFailed` with a non-secret message. Bodies, keys and reasons are never logged.

   Rejected: awaiting in `handle`, which would serialise everything behind one slow request.
10. **Q10: markdown on send.** RECOMMENDED: a pure core function `outgoing_content(&OutgoingMessage) -> RoomMessageEventContentWithoutRelation`.
    - Each `MentionRef` label is replaced at its first remaining occurrence by `[escaped label](https://matrix.to/#/<percent-encoded user id>)`.
    - The result goes through `text_markdown` (or `emote_markdown` / `notice_markdown` per `kind`).
    - `body` is then overwritten with the plain text: labels without link syntax, the original text otherwise.
    - `add_mentions(Mentions::with_user_ids(..))` lists every mentioned user still present.
    - Raw inline HTML typed by the user passes through as pulldown-cmark emits it. Every receiver, including our own echo, sanitizes it (Q4/Q5).

    Rejected: escaping `<` in the source (breaks code spans), and HTML-building in the app.
11. **Q11: local echo, retry and discard.** RECOMMENDED:
    - The SDK `SendQueue` drives `sent: SendStatus::{Sending, Failed, Sent}` on the same item ID; `send_failure.recoverable` comes from `SendingFailed { is_recoverable }`.
    - `RetrySend`: `unwedge()` if the item is wedged, then `room.send_queue().set_enabled(true)`.
    - `DiscardSend`: `abort()`, then `set_enabled(true)`, so that later queued messages proceed.
    - When `SyncState` returns to `Running` after `Offline` or `Reconnecting`, the core calls `client.send_queue().set_enabled(true)` so recoverable failures retry on their own.
    - The UI shows pending items muted with a clock mark and failed items in the danger colour, with **Retry** and **Discard**.

    Rejected: hiding failed echoes, and auto-retrying wedged (unrecoverable) requests.
12. **Q12: read receipts.** RECOMMENDED:
    - **Sent:**
      - When the bottom row is visible, the newest item ID differs from the last one marked, and the window has had focus since, the app sends `MarkRead { fully_read: false }`. It's debounced to at most once per second through a `Task` timer on the bridge runtime. The core calls `mark_as_read(ReceiptType::Read)`.
      - On deselecting a room whose bottom was seen, the app sends `MarkRead { fully_read: true }`, which gives `mark_as_read(FullyRead)` and moves the read marker the way Element does.
    - **Shown:**
      - `read_by` (other users only) under the item they point at: up to 3 short names plus "+N";
      - the SDK `ReadMarker` as a "New messages" line.

    Rejected: private receipts by default (a settings toggle is later scope), and a receipt for every item scrolled past.
13. **Q13: typing notifications.** RECOMMENDED:
    - Sent:
      - `SetTyping { typing: true }` on an edit that leaves the composer non-empty, at most once every 3 s (the SDK throttles too);
      - `typing: false` when the composer is emptied, on send, and on deselect.
    - Received: on select the core subscribes with `subscribe_to_typing_notifications` and emits `Typing { users }`, with names taken from `members_no_sync`. The app shows "Alice is typing…", "Alice and Bob are typing…" or "Several people are typing…" above the composer.
    - The core also calls `set_room_subscriptions(&[selected])` on select, so typing and receipts arrive for the open room through sliding sync.

    Rejected: a typing-off setting (later scope).
14. **Q14: virtualization.** RECOMMENDED:
    - The timeline is a `scrollable(...).anchor_bottom()` with `Id::new("timeline")`. Its offset is the distance from the bottom (see Facts).
    - Row heights come from `sensor(row).key(item_id).anticipate(OVERSCAN_PX).on_show/on_resize`, which report `Message::RowMeasured { id, height }`, stored in `HashMap<String, f32>`. They're cleared when the pane width changes by more than 1 px.
    - Unmeasured rows use a pure `estimate_height(item, continuation, width)`: header, wrapped lines (chars per line from width), reactions row and receipts row.
    - A prefix-sum `Vec<f32>` is rebuilt after each diff batch or measurement (O(n)). From it a pure `visible_window(prefix, offset_from_bottom, viewport_h, overscan_px)` gives the index range. Only that range is built, between two `space()` spacers that keep the full content height.
    - **Anchor rule (scroll stability):**
      - while the offset from the bottom is above 2 px, the app records the top visible item's ID and its pixel position in the viewport;
      - after any diff batch or measurement, a pure `anchor_correction(old, new) -> Option<f32>` gives the new bottom offset, and the app issues `operation::scroll_to(timeline_id, AbsoluteOffset { x: None, y: Some(new) })` when it differs by more than 0.5 px;
      - at the bottom the view follows new messages, with no correction.

      Prepends need no correction (bottom anchor). Appends or height changes below the viewport are corrected by this rule.
    - Constants: `OVERSCAN_PX = 600`, `MAX_BUILT_ROWS = 120` (a hard cap in case of tiny estimates).

    Rejected:
    - fixed row heights (the Phase 2 room list's approach; messages vary too much);
    - building all rows;
    - iced `lazy` (doesn't skip layout);
    - a custom virtual-list widget (needs `advanced` in the normal build, and more code to maintain).
15. **Q15: proof of virtualization and timing.** RECOMMENDED:
    - **(a) Timing.** A plain `#[test]` (Phase 2 Q17 style) named `timeline_batches_under_16ms`, on deterministic synthetic items: 10,000 items, a divider every 50, a state event every 25, every third message with a 3-block `RichText`, every fifth with 2 reaction groups. Batch shapes, each through `update(Message::Core(TimelineDiff{..}))` including prefix-sum and window bookkeeping:
      1. `Reset` of 10,000;
      2. 20 × `PushFront` plus 1 `Insert` (a back-pagination) on 10,000;
      3. `PushBack` of 5 on 10,000 while scrolled up (with the anchor correction);
      4. 100 `Set` (reaction and receipt updates) plus 10 `Remove`/`Insert` pairs;
      5. `Clear` then `Reset` of 10,000.

      For each shape: median of 15 runs after one warm-up, under 16 ms, printed with `--nocapture` and recorded in PROGRESS.md.
    - **(b) View build.** Building `timeline_pane::view` for a 10,000-item state has a median under 16 ms, and a test-only counter shows at most `MAX_BUILT_ROWS` rows built.
    - **(c) Headless layout.** With the dev-only iced `advanced` feature: lay out the timeline pane for a 10,000-item state at 800×600 on the headless tiny-skia renderer. The scrollable's content column has `window.len() + 2` children, and that's at most 60 at the default estimates. The content height equals the prefix-sum total within 1 px.

      If the headless renderer can't be created on a CI OS (`None`), the test fails and the implementer stops and asks (rule 8). It isn't skipped silently.

    Rejected: a release-only bench, `criterion`, and `iced_test` (a new dependency).
16. **Q16: back-pagination.** RECOMMENDED:
    - The app sends `PaginateBackwards` when the top of the built window is within one viewport height of the content top, the room's `PaginationState` is `Idle`, and the timeline is the selected room's. It also sends it once after the first `Reset` if the content is shorter than the viewport.
    - The core calls `paginate_backwards(20)` and forwards `live_back_pagination_status()` as `Pagination` events (`Idle { hit_timeline_start: true }` gives `ReachedStart`).
    - The app shows a small spinner row while `Loading` and the SDK's `TimelineStart` as "Beginning of the room".

    Rejected: page sizes above 50 (longer stalls on slow servers).
17. **Q17: composer.** RECOMMENDED:
    - `text_editor(&state.composer)` with `.key_binding`:
      - Enter without Shift sends (`Binding::Custom(ComposerSend)`), unless a suggestion list is open, in which case Enter and Tab pick;
      - Shift+Enter gives `Binding::Enter`;
      - Up and Down move in an open suggestion list, and Escape closes it or cancels reply/edit;
      - everything else is `Binding::from_key_press`.
    - Editor actions travel as `Message::Composer(EditorAction)`, a newtype whose `Debug` shows only the action kind, because iced's `Edit::Paste` `Debug` shows the text.
    - `App`'s manual `Debug` never prints `Content`, which also dumps the buffer.
    - Sending trims trailing blank lines and refuses blank text. A `/me ` prefix gives `MessageKind::Emote`.
    - One composer per window, cleared on room switch, as in Phase 2.
    - IME composition is left to iced and checked by the human.
    - Residual, extending the Phase 2 Q19 issue: iced's editor buffer isn't zeroized.

    Rejected: per-room drafts (scope; Q24), and a custom editor widget.
18. **Q18: autocomplete and emoji.** RECOMMENDED:
    - **Mentions:**
      - typing `@` plus at least 1 char, at a word start before the cursor, sends `SearchMembers { query }` at most every 150 ms;
      - the core answers with up to 8 joined members whose display name or user ID contains the query (case-insensitive), from `members_no_sync(JOIN)` after one `sync_members()` per selection;
      - picking a member inserts the sanitized display name (or the localpart) plus a space, and records a `MentionRef`.
    - **Emoji:** `:` plus at least 2 chars from `[a-z0-9_+-]` at a word start lists up to 8 `emojis::iter()` matches by shortcode prefix. Picking one, or typing the closing `:` of an exact shortcode, inserts the emoji.
    - **Reaction picker:** a quick row of 👍 ❤️ 😂 😮 😢 🎉 👀 ✅, a search field (shortcode or name substring, up to 64 results), and a grid by `Group`.

    Rejected: shipping the whole member list to the app (big rooms have tens of thousands of members), and custom (image) emoji.
19. **Q19: links and pills.** RECOMMENDED:
    - **Core** (`rich_text.rs`): `href`s are parsed with `url::Url`, already a core dependency.
      - Kept: `https`, `http` and `mailto` with no userinfo and at most 2,048 bytes. They're serialized normalized (punycode host, percent-encoded path).
      - Anything else (`javascript:`, `data:`, `file:`, custom schemes, unparsable) becomes plain text.
      - `MatrixToUri::parse` / `MatrixUri::parse` successes become `Mention`: `User` for `@`, and `Room` for `!` or `#` (an event permalink becomes a room pill).
      - Pill labels are the anchor text (sanitized). An empty anchor falls back to the ID.
    - **App:**
      - `LinkTarget::{Url(String), User(UserId), Room(String), Spoiler(usize)}` is the `rich_text` link type;
      - clicking a `Url` re-checks it with a pure `check_link_url` (ASCII-only, scheme allow-list, no userinfo, no control or space characters; modelled on Phase 2 `check_browser_url`) and opens it with `open::that_detached` inside a `Task`, never in `update`;
      - if the visible link text looks like a URL whose host differs from the target host (pure `needs_confirmation`), a confirmation bar "Open <host>?" appears first;
      - user pills do nothing yet (profiles are Phase 5), and room pills send `SelectRoom` when the room is in the list.
    - URLs are never logged.

    Rejected: opening without the app-side re-check, a confirmation for every link, and following `matrix:` links to join rooms (Phase 5).
20. **Q20: untrusted text in the timeline.** RECOMMENDED:
    - `text_safety` gains `multi_line_capped(text, max_chars, max_lines)`. It applies the same rules as `one_line` except that `\n` (and CR, NEL, LS, PS, normalized to `\n`) is kept, up to `max_lines`.
    - Every `Inline` text, label and code block goes through it in the app. Names, reply previews, reaction keys (32-char cap) and typing names use `one_line_capped` or `name_or_placeholder`.
    - Bodies render only inside the indented content column, below a bold sender header. A body can't take the header's place, though it can contain bold text that looks like a sender name; that residual risk is shared with every client and documented.
    - **Copy** puts the raw `body` on the clipboard: user-initiated, never rendered, never logged.
    - New `Debug` impls are redaction-tested with sentinel strings.

    - The new function reuses the existing hand-written ranges (the combining-mark list, the invisible and format characters, the joiner and selector rules). No Unicode general-category crate is used: the human rejected it on 2026-10-02 (Q1).

    Rejected:
    - keeping bodies on one line (multi-line is the point of Phase 3);
    - a Unicode general-category crate (rejected by the human).
21. **Q21: dates and times.** RECOMMENDED:
    - Pure functions generic over `chrono::TimeZone`, so tests use `FixedOffset` and a fixed "now":
      - `day_label(ts, now, tz)` gives "Today", "Yesterday", a weekday name within the last 6 days, `2 October` in the current year, or `2 October 2025` otherwise;
      - `time_label(ts, tz)` gives `HH:MM`.
    - Production uses `chrono::Local`. English only (i18n is out of scope).

    Rejected: UTC labels, and formatting in the core.
22. **Q22: grouping.** RECOMMENDED: a pure `is_continuation(items, i)`, true when item `i` and the nearest earlier item are both `Message` or `Placeholder` from the same sender, are less than 5 minutes apart, and nothing else lies between them (no `DateDivider`, `ReadMarker`, `State` or `TimelineStart`). Continuations have no header and a tighter top padding. It's computed per built row in `view`, O(1), with no stored array. Rejected: grouping in the core.
23. **Q23: state events and placeholders.** RECOMMENDED:
    - `StateChange::{Joined, Left, Invited { target }, Kicked { target }, Banned { target }, Unbanned { target }, InviteAccepted, InviteRejected, InviteRevoked { target }, Knocked, KnockAccepted { target }, KnockRetracted, KnockDenied { target }, DisplayName { old, new }, Avatar, RoomCreated, RoomName { name }, Topic, RoomAvatar, Encryption, Other { event_type }}`, where targets carry an optional display name.
    - Each renders as one muted line, for example "Alice joined", "Alice changed their name to Ally" or "Bob changed the room name to Lounge".
    - `MembershipChange::{None, Error, NotImplemented}` and failed-to-parse events become `Other`.
    - Placeholders render muted, for example "Message deleted", "Unable to decrypt this message", "sent an image (not shown yet)".
    - Collapsing runs of membership events is out of scope (Q24).
24. **Q24: scope cuts.** RECOMMENDED, out of Phase 3:
    - rendering and sending media (images, files);
    - threads (thread replies show as normal messages with their reply quote);
    - per-room drafts;
    - edit history;
    - collapsing state-event runs;
    - link previews;
    - custom emoji;
    - slash commands other than `/me`;
    - user profiles from pills;
    - E2EE flows (Phase 4);
    - joining rooms from pills (Phase 5);
    - private receipts and typing-off settings;
    - desktop notifications;
    - i18n.

    No changes to existing pins.
25. **Q25: integration tests and CI.** RECOMMENDED:
    - New ignored test files `crates/core/tests/timeline_items.rs`, `timeline_actions.rs` and `timeline_live.rs` (Tasks 3-7). They use alice, carol and the existing helpers. bob stays reserved (timeline.rs note). Rooms get unique names, and the ignore reason is verbatim.
    - The `integration` CI job has a 20-minute timeout: each task records the ignored-run duration in PROGRESS.md. If the run passes 15 minutes, stop and ask (rule 8) rather than raise the timeout.
    - A 10,000-event room for the human checkpoint is seeded by a core example, `crates/core/examples/seed_big_room.rs`. It reads `.env.test`, refuses non-local homeservers with the same loopback check as the test helpers, sends in batches, and prints only the room ID. It's compiled by `cargo test` and never run in CI.

    Rejected: a 10,000-event seed inside the ignored test run (minutes per CI run).

### Conventions for every task
- **Gate (rule 4)**, required before each commit:
  - `cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && cargo test --workspace --locked`, plus `scripts/check-app-deps.sh`, `scripts/check-core-api.sh` and `cargo deny check`.
  - Tasks that touch the core or the bridge also run `docker compose up -d --wait && scripts/create-test-users.sh && cargo test -p twicecat-core -p twicecat-app --locked -- --ignored --skip keyring`, then `docker compose stop`.
  - The agent never runs the real-keychain test.
- **One commit per task**, message `phase3.<task number>: <summary>`, with the attribution trailer as instructed by the session. ROADMAP.md is never committed.
- **PROGRESS.md (rule 5).**
  - Task 1 adds a "Phase 3 steps" checklist (one line per task, with the roadmap steps it covers), the dependency lines under "Pinned versions", and a "Phase 3 human checkpoint" list.
  - Each task ticks its line and adds a "Phase 3 / Task N notes" block with:
    - the checks run and their results;
    - the ignored-run duration;
    - every matrix-sdk(-ui), ruma, iced, `emojis`, `chrono` or `open` API used, with the registry file:line where it was confirmed;
    - open issues and new human items.
  - Rendering, scrolling feel, IME, link opening and OS behaviour go on the human list, never ticked by the agent (rule 9).
- **Boundary (rule 7).**
  - `crates/app` depends only on `twicecat-core`, iced, image, tokio, open, emojis and chrono. It imports only `twicecat_core::{api, Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE, paths, secrets, logging}`.
  - `crates/core/src/api/` stays SDK-free (`check-core-api.sh`). `rich_text.rs`, `outgoing.rs` and the timeline mapping stay private core modules.
- **No network or blocking in `update`.** `update` only mutates state, calls `try_send`, and returns `Task`s, including scroll operations, link opening and debounce timers. Unit tests drive `update` with a plain `mpsc` sink and no runtime.
- **Secrets and content hygiene (rule 6).**
  - No token, password, message body, formatted body, URL, reaction key, redaction reason or composer text reaches `tracing`, a file, or a `Debug` string.
  - Every new public type that holds one has a redacted `Debug`, unit-tested with a sentinel value.
- **Stop and ask (rule 8)** if:
  - any API above behaves differently from this plan;
  - a test is flaky twice in a row;
  - the headless renderer is unavailable on a CI OS;
  - the integration run passes 15 minutes;
  - a step needs a real account.

### Task 1: Phase 3 dependencies, advisory triage, ledger
Language: rust
Files: crates/core/Cargo.toml, crates/app/Cargo.toml, Cargo.lock, deny.toml, PROGRESS.md
Depends on: none (Q1: Phase 2 signed off by the human on 2026-10-02)
Acceptance criteria:
- The "Phase 2 human checkpoint" section of PROGRESS.md records the human's 2026-10-02 decisions:
  - the CI item is ticked;
  - "Approve the additive API (Q6) as the Phase 3 baseline" is ticked;
  - the Unicode general-category crate item is ticked as "rejected by the human; text_safety keeps hand-written ranges";
  - the note "Phase 2 signed off by the human on 2026-10-02 (merge of PR #3)" is added;
  - every other unticked Phase 2 item stays unticked as a known open check.
- `crates/core/Cargo.toml` changes only by adding `"markdown"` to matrix-sdk's features; the pin `=0.19.1` is unchanged.
- `crates/app/Cargo.toml` gains exactly the Q2 entries: `emojis`, `chrono`, and the dev-only `iced` entry with `advanced`.
- The `Cargo.lock` diff adds exactly pulldown-cmark 0.13.4, pulldown-cmark-escape 0.11.0 and unicase 2.9.0, plus dependency edges. No existing version changes (checked with a sorted name/version comparison recorded in PROGRESS.md).
- `cargo tree -p twicecat-app --depth 1 -e normal` lists emojis v0.9.0 and chrono v0.4.45. `cargo tree -p twicecat-app -e features -i iced` shows `advanced` only under dev.
- `deny.toml` gains only the Q3 ignore entry. `cargo deny check` passes, and PROGRESS.md records that the advisory failure was pre-existing on `main`.
- `scripts/check-app-deps.sh` exits 0.
- PROGRESS.md gains the Phase 3 steps list, the pinned-versions lines, and a "Phase 3 human checkpoint" list seeded with:
  - CI green on Linux, Windows and macOS with the new dependencies;
  - revisit Q1-Q25.
- Gate passes (unused dependencies are allowed in this commit). Commit: `phase3.1: phase 3 dependencies, advisory triage and ledger`.

### Task 2: Additive core API for Phase 3
Language: rust
Files: crates/core/src/api/mod.rs, crates/core/src/api/types.rs, crates/core/src/timeline.rs, crates/core/src/engine.rs, crates/core/src/bin/twicecat-cli.rs, crates/core/tests/timeline.rs, crates/app/src/screens/main_view.rs, crates/app/src/room_list.rs, PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- The API has exactly the Q7 additions, all `Clone + Debug + PartialEq`, SDK-free and documented. `scripts/check-core-api.sh` exits 0.
- `TimelineItem::new(id, kind)` gives an empty `read_by`, and `MessageDetails::default()` an empty `RichText` with every flag `false`. Every existing struct literal (core mapping, CLI, core and app tests) compiles through them, and the existing tests pass unchanged in meaning.
- Redaction unit tests with sentinel values (`secret-body-x`, `secret-key-x`, `secret-url-x`, `secret-reason-x`). No sentinel appears in `format!("{:?}")` of:
  - a `Message` item whose `RichText` holds them in text, link URL and mention label;
  - a `ReplyPreview`, a `ReactionGroup` key, an `OutgoingMessage`;
  - `Command::Edit`, `Command::Redact { reason }` and `Command::ToggleReaction`;
  - a `CoreEvent::TimelineDiff` carrying any of them.
- The engine accepts every new command without panicking, before the real handlers land:
  - with no selected room or matching room, each answers `ActionFailed { action, error: NoRoomSelected }` (or `NotLoggedIn` with no session);
  - with a matching selected room, each still answers `ActionFailed` with `Internal("not implemented yet")`;
  - `SearchMembers` answers `Members { members: [] }`.

  Each case is unit-tested with the engine's test helper (no network).
- The CLI handles `Pagination`, `Typing`, `Members` and `ActionFailed` by ignoring them (exhaustive match kept) and still prints text messages. Its tests pass.
- Gate and the ignored run pass. Commit: `phase3.2: additive core api for timeline items, actions and composer`.

### Task 3: Timeline item view models in the core (roadmap steps 1, 7, 8 and 9, core side)
Language: rust
Files: crates/core/src/timeline.rs, crates/core/src/engine.rs, crates/core/tests/common/mod.rs, crates/core/tests/timeline_items.rs, PROGRESS.md
Depends on: Task 2
Acceptance criteria:
- `map_item` fills the Q7 model:
  - virtual items become `DateDivider` (ms), `ReadMarker` and `TimelineStart`;
  - `m.text`, `m.notice` and `m.emote` become `Message` with `details`:
    - `sender_name` and `sender_name_ambiguous` from a `Ready` profile;
    - `kind`, `edited`, `highlighted`;
    - `reply_to` from `in_reply_to` (`Ready`, `Pending` or `Unavailable` mapped to `ReplyState`; body is the first line, at most 200 chars);
    - `reactions`: one group per key in SDK order, `count` = senders, `own` = own user among them, at most 5 senders kept;
    - `actions` from `can_be_replied_to`, `is_editable`, own-or-remote for redact, and remote-or-local for react;
    - `send_failure` from `SendingFailed`.
  - membership, profile and other state become `State` per Q23;
  - redacted, UTD, media, sticker, poll and location become `Placeholder`;
  - `read_by` comes from `read_receipts()`, minus the own user, at most 20.
- `content` is the plain-body `RichText` (paragraphs split on `\n`); Task 4 adds HTML.
- Pure helpers have table-driven unit tests:
  - `map_membership(MembershipChange) -> StateChange`, all 17 variants;
  - `reaction_groups(..)`, which caps at 5 senders and sets the `own` flag;
  - `reply_body(..)`, which handles multi-byte text at the 200-char cap;
  - `receipts_without_own(..)`.
- Test helpers gain `set_display_name(client, name)` and `send_text(client, room, body) -> event id`. They never print bodies or tokens.
- Integration tests (verbatim ignore reason):
  - **state items:** alice creates a room and invites carol, carol joins, and carol changes her display name. Within 30 s, alice's applied timeline holds `State` items `Invited`, `Joined` and `DisplayName { new }` in that order, plus at least one `DateDivider`;
  - **message details:** carol sends a notice and an emote. Alice sees `Message` items with kinds `Notice` and `Emote`, with `sender_name` set.
- Gate and the ignored run pass. Commit: `phase3.3: rich timeline item view models in the core`.

### Task 4: HTML to rich text mapper in the core (roadmap step 4, core side)
Language: rust
Files: crates/core/src/rich_text.rs, crates/core/src/lib.rs, crates/core/src/timeline.rs, crates/core/tests/fixtures/html/*.html, crates/core/tests/timeline_items.rs, PROGRESS.md
Depends on: Task 3
Acceptance criteria:
- `rich_text.rs` is a private module: `pub(crate) fn from_html(html: &str, body: &str) -> RichText` and `from_plain(body) -> RichText`, per Q4, Q5, Q6 and Q19.
- The source selection per Q4 (raw `latest_json` formatted body, then the SDK formatted body, then the plain body) is a pure function. It's unit-tested with hand-built JSON for an original event, an edit (`m.new_content`), a non-HTML `format`, and a missing body.
- **Fixture tests** (one `.html` input plus an expected `RichText` built in the test, per case):
  - **allowed tags kept:** bold, italic, strike, underline, inline code, `pre`/`code` with whitespace, links, `blockquote` nested twice, `ul`, `ol start=3` with nested list, `h2`, `br`, `hr`;
  - **stripped:** `script` and `style`, **with their contents** (no `alert`, no CSS text in any `Inline`), plus `iframe`, `svg` and `mx-reply`;
  - unknown elements unwrapped with text kept; colours ignored;
  - **mention pills:** `https://matrix.to/#/@alice:localhost` (also percent-encoded) gives `Mention(User)` with the anchor label; `#room:localhost`, `!id:localhost` and an event permalink give `Mention(Room)`; `matrix:u/alice:localhost` gives `Mention(User)`;
  - **links:** `javascript:`, `data:`, `file:`, `vbscript:`, userinfo and an over-long URL become plain text; `HTTPS://Example.ORG/ä` is normalized;
  - **fallback to `body`:** no formatted body, `format` not HTML, a formatted body that maps to blank text, and unparsable markup each give the plain body, linkified;
  - **caps:** depth 40 is flattened at 32; 3,000 inlines are capped at 2,000; 20,000 chars are cut with an ellipsis; nothing panics on malformed HTML (unclosed tags, stray `</li>`, NUL characters).
- **Linkify** unit tests: trailing punctuation and parentheses, two URLs in one line, a URL-only body, `http://` kept, `ftp://` not linked.
- `map_item` uses `from_html` for messages. The Task 3 tests still pass.
- Integration test (verbatim ignore reason): carol sends, through the raw SDK helper, `text_html("ok", "<b>ok</b><script>alert(1)</script><a href=\"https://matrix.to/#/@alice:localhost\">Alice</a>")`. Alice's item has a bold `ok`, a `Mention(User(@alice:localhost))`, and no inline text containing `alert`.
- Gate and the ignored run pass. Commit: `phase3.4: html formatted body to rich text in the core`.

### Task 5: Sending: markdown, mentions, replies, local echo retry and discard (roadmap steps 6, 7 and 9, core side)
Language: rust
Files: crates/core/src/outgoing.rs, crates/core/src/lib.rs, crates/core/src/engine.rs, crates/core/tests/timeline_actions.rs, PROGRESS.md
Depends on: Task 4
Acceptance criteria:
- `outgoing.rs` (private) has `outgoing_content(&OutgoingMessage) -> RoomMessageEventContentWithoutRelation` per Q10. **Markdown-to-HTML unit tests:**
  - `hello` stays plain (no `formatted`);
  - `**bold** and _it_` gives `<strong>bold</strong> and <em>it</em>`;
  - a fenced code block gives `<pre><code>`;
  - `a\nb` gives a `<br />`;
  - a mention of label `Alice [x]` gives a link to `https://matrix.to/#/%40alice%3Alocalhost` (or the ruma-accepted encoding, recorded), a plain `body` of `Alice [x]`, and `m.mentions.user_ids == [@alice:localhost]`;
  - a mention label no longer in the text is dropped from `m.mentions`;
  - `/me waves` with `kind: Emote` gives `m.emote`.
- `Command::Send` runs in the selection's `JoinSet` (Q9):
  - with `reply_to`, the core resolves the item's event ID and calls `send_reply`; otherwise it calls `timeline.send`;
  - `NotAllowed` when `can_be_replied_to` is false;
  - `ActionFailed { action: Send }` on error.
- `RetrySend` and `DiscardSend` follow Q11. On `SyncState::Running` after `Offline` or `Reconnecting`, the engine calls `client.send_queue().set_enabled(true)` (unit-tested through the existing supervisor test seam or a helper).
- `Selected` becomes `{ room_id, timeline: Arc<Timeline>, tasks: JoinSet<()>, .. }` and `stop()` aborts the tasks. The existing timeline tests still pass.
- Integration tests (verbatim ignore reason; alice sends, carol receives):
  - **markdown round trip:** `Send { text: "**hi** `x`" }` gives carol an item whose `RichText` has bold `hi` and code `x`;
  - **reply round trip:** alice replies to carol's message, and carol's item has `reply_to.sender == carol` and a `body` equal to the original's first line;
  - **mention:** a mention of carol shows on carol's side with `highlighted == true` and a `Mention(User(carol))` inline;
  - **local echo:** right after `Send`, alice's timeline holds the item with `Sending` and then `Sent` under the same item ID within 30 s;
  - **failure and discard:** a 70,000-byte message gives `sent == Failed` with `recoverable == false` (Synapse `M_TOO_LARGE`; if the SDK classifies it differently, stop and ask). `DiscardSend` removes it, and a following normal `Send` still arrives (queue re-enabled).
- Gate and the ignored run pass. Commit: `phase3.5: markdown sending, mentions, replies and local echo recovery`.

### Task 6: Edit, redact, react and member search in the core (roadmap steps 6 and 7, core side)
Language: rust
Files: crates/core/src/engine.rs, crates/core/src/timeline.rs, crates/core/tests/timeline_actions.rs, PROGRESS.md
Depends on: Task 5
Acceptance criteria:
- `Edit` gives `timeline.edit(&identifier, EditedContent::RoomMessage(outgoing_content(..)))`, and only for `is_editable` items (else `NotAllowed`).
- `Redact` gives `timeline.redact(&identifier, reason)`. The reason is exposed only at the call.
- `ToggleReaction` gives `timeline.toggle_reaction(&identifier, key)`. Keys are trimmed and capped at 64 bytes, and an empty key is refused with `NotAllowed`.
- `SearchMembers` runs `sync_members()` once per selection, then filters `members_no_sync(JOIN)` per Q18. It returns at most 8, sorted by "starts with" before "contains", then by name, in a pure, unit-tested `rank_members`.
- Every failure gives `ActionFailed` with the matching `ActionKind`.
- Integration round trips (verbatim ignore reason):
  - **edit:** alice edits her message to `**new**`. Carol's item keeps its item ID, and gets `edited == true` and a bold `new`;
  - **redact:** alice redacts her message, and carol's item becomes `Placeholder { what: Redacted }`;
  - **react:** carol reacts 👍 to alice's message. Alice sees `reactions == [{ key: "👍", count: 1, own: false }]`. Carol sees `own: true`. A second toggle by carol removes the group on both sides;
  - **members:** `SearchMembers { query: "car" }` from alice returns carol.
- Gate and the ignored run pass. Commit: `phase3.6: edit, redact, react and member search in the core`.

### Task 7: Back-pagination, room subscription, receipts and typing in the core (roadmap steps 3 and 8, core side)
Language: rust
Files: crates/core/src/engine.rs, crates/core/tests/timeline_live.rs, crates/core/examples/seed_big_room.rs, PROGRESS.md
Depends on: Task 6
Acceptance criteria:
- `SelectRoom` also does three things:
  - calls `room_list_service().set_room_subscriptions(&[id])` (when sync is running);
  - starts the pagination-status forwarder, which emits `Pagination` per Q16 (the first value comes at once);
  - starts the typing forwarder, which emits `Typing` per Q13 with names from members.

  Both forwarders end with the selection.
- `PaginateBackwards` calls `paginate_backwards(20)` in the `JoinSet`, at most one at a time (unit-tested with a flag helper).
- `MarkRead { fully_read }` calls `mark_as_read(Read | FullyRead)`.
- `SetTyping` calls `typing_notice`.
- Integration tests (verbatim ignore reason):
  - **pagination:** the helper alice sends 60 numbered messages to a new room. A fresh alice core selects it and repeats `PaginateBackwards` on each `Idle`. Within 60 s it reaches `ReachedStart`, and the applied timeline holds all 60 in order, with the room's `TimelineStart` first;
  - **receipts:** carol selects alice's room and sends `MarkRead { fully_read: false }`. Within 30 s alice's newest item has `read_by` containing carol;
  - **typing:** alice sends `SetTyping { typing: true }`. Carol, with the room selected, receives `Typing { users: [alice] }`, and then `[]` after `typing: false`.
- `examples/seed_big_room.rs` (Q25) compiles under `cargo test`. Running it is a human step: it's documented in PROGRESS.md with the exact command, and refuses a non-local `HOMESERVER_URL`.
- Gate and the ignored run pass. Commit: `phase3.7: back-pagination, room subscription, receipts and typing in the core`.

### Task 8: Timeline state and virtualization model in the app, 10,000-item timing test (roadmap step 2, model)
Language: rust
Files: crates/app/src/timeline.rs, crates/app/src/lib.rs, crates/app/src/state.rs, PROGRESS.md
Depends on: Task 7
Acceptance criteria:
- `timeline.rs` holds `TimelineView`:
  - `items: Vec<TimelineItem>`, `measured: HashMap<String, f32>`, `prefix: Vec<f32>`;
  - `offset_from_bottom`, `viewport_h`, `width`, `anchor: Option<(String, f32)>`;
  - `pagination: PaginationState`, `last_marked: Option<String>`.

  `App.timeline` moves into it, and the Phase 2 tests are adapted.
- Pure functions with unit tests:
  - `estimate_height` (header vs continuation, wrapping by width, reactions and receipts rows; monotonic in text length);
  - `is_continuation` (Q22: same sender under 5 min, a divider or state in between, a different sender, placeholders);
  - `rebuild_prefix`;
  - `visible_window` (bottom, middle and top, empty, viewport taller than content, a stale offset, NaN or negative input; never more than `MAX_BUILT_ROWS`);
  - `spacer_heights` (the content height is preserved);
  - `anchor_correction` (prepend: no change; append while scrolled up: offset grows by the appended height; an item below the anchor growing by 40 px: offset grows by 40; at the bottom: `None`; anchor item removed: falls back to the nearest surviving item);
  - `should_paginate` (Q16 table).
- `update` handles `TimelineDiff` (selected room only), `RowMeasured`, `TimelineScrolled(Viewport)` and `Pagination`.
  - It returns a scroll `Task` when `anchor_correction` asks for one. Tests check this through a state field recording the last requested offset, because `Task`s are opaque.
  - It sends `PaginateBackwards` per Q16 at most once per `Idle` period (unit test with a scripted scroll sequence).
- **Timing (Q15a):** `timeline_batches_under_16ms` with the 5 shapes, median of 15 under 16 ms each, timings recorded in PROGRESS.md.
- Gate passes. Commit: `phase3.8: virtualized timeline model with 10,000-item timing test`.

### Task 9: Timeline pane rendering, measurement and the visible-only layout test (roadmap steps 1, 2 and 3, UI)
Language: rust
Files: crates/app/src/screens/timeline_pane.rs, crates/app/src/screens/mod.rs, crates/app/src/screens/main_view.rs, crates/app/src/state.rs, crates/app/src/text_safety.rs, crates/app/src/dates.rs, crates/app/src/theme.rs, crates/app/tests/timeline_layout.rs, PROGRESS.md
Depends on: Task 8
Acceptance criteria:
- `main_view::room_pane` uses `timeline_pane::view`. `last_messages`, `display_lines` and `MAX_MESSAGES` are removed together with their tests, replaced by the new ones.
- `timeline_pane::view` builds only the `visible_window` rows between spacers, inside `scrollable(..).id(TIMELINE_ID).anchor_bottom().on_scroll(..)`. Each row sits in a `sensor` keyed by item ID, with `anticipate(OVERSCAN_PX)`, `on_show` and `on_resize`.
- Rows:
  - message header (sanitized sender name or user ID, bold; `time_label`), with body text for now as plain sanitized multi-line text (Task 10 adds rich text);
  - continuation rows with no header;
  - `DateDivider` with `day_label`; `ReadMarker` as "New messages" in the accent colour; `TimelineStart`; a "Loading…" row while paginating;
  - `State` and `Placeholder` muted, per Q23;
  - highlighted messages on a highlight background.
- `text_safety::multi_line_capped` is added per Q20, with unit tests: newline variants normalized, bidi replaced, line and char caps, and multi-byte boundaries.
- `dates.rs` holds `day_label` and `time_label` per Q21, with unit tests in fixed time zones (+14:00, -12:00) across midnight and year boundaries.
- **Headless layout (Q15c):** `tests/timeline_layout.rs` lays out the pane for a 10,000-item state at 800×600 with the headless tiny-skia renderer. It asserts at most 60 built rows plus 2 spacers, and a content height within 1 px of the prefix total. With the offset moved to the middle, the window covers the expected index range.
- **View build (Q15b):** a test-only counter shows at most `MAX_BUILT_ROWS` rows built for 10,000 items, with a build median under 16 ms.
- The Task 8 timing test still passes.
- Human checklist gains:
  - scrolling a 10,000-event room (seeded with the Task 7 example) stays smooth, with no blank gaps when dragging the scrollbar fast;
  - the scroll position holds while older messages load at the top;
  - reading history while new messages arrive doesn't jump;
  - rows look right in both themes.
- Gate passes. Commit: `phase3.9: virtualized timeline pane with day separators, grouping and state events`.

### Task 10: Rich text rendering, link handling and pills (roadmap steps 4 and 5, UI)
Language: rust
Files: crates/app/src/rich_text.rs, crates/app/src/links.rs, crates/app/src/lib.rs, crates/app/src/screens/timeline_pane.rs, crates/app/src/state.rs, PROGRESS.md
Depends on: Task 9
Acceptance criteria:
- `rich_text.rs` maps `RichText` to widgets:
  - each `Block` becomes a `column` entry, and each paragraph or heading becomes one `rich_text` with `Span`s;
  - text style maps to bold, italic (`font::Style::Italic`), strikethrough, underline, and code (`Font::MONOSPACE` on a subtle background);
  - `Code` becomes a monospace block in a bordered `container`, `Quote` gets a left bar and indent, and lists use bullets or numbers from `start`, with nesting indented;
  - `Mention` becomes a pill span (background, border radius, padding), with a stronger colour when it names the own user;
  - `Spoiler` becomes a pill reading "Spoiler" until clicked (state per item and index).

  Every text goes through `multi_line_capped` or `one_line_capped` (Q20).
- Pure `spans_for(&[Inline], theme) -> Vec<SpanModel>` (an app-side plain struct, so it's testable without a renderer). **App-side fixture tests** reuse the Task 4 shapes:
  - styles combine (bold plus italic plus link);
  - a mention gives a pill with `LinkTarget::User`;
  - a bidi override in link text is replaced;
  - the plain-body fallback renders as paragraphs.
- `links.rs`:
  - `check_link_url` (https, http and mailto OK; `javascript:`, `file:`, `data:`, userinfo, spaces, controls and non-ASCII refused);
  - `needs_confirmation(text, url)` (a different host gives true; same host, non-URL text and mailto give false);
  - `open_link` is reachable only from a `Task`.

  All unit-tested. A grep-style test checks that `update` never calls `open::` directly.
- `Message::LinkClicked(LinkTarget)`:
  - `Url` gives a check, then confirm or open;
  - `Room` gives `SelectRoom` when the room is listed;
  - `User` gives nothing;
  - `Spoiler` toggles reveal.

  The confirmation bar shows only the host. URLs are never logged.
- Human checklist gains:
  - formatted messages from Element (bold, italic, code blocks, quotes, nested lists, headings) look right in both themes;
  - links open in the browser on each OS;
  - a mismatched-host link asks first;
  - pills render and room pills open the room;
  - spoilers reveal on click.
- Gate passes. Commit: `phase3.10: rich text rendering, safe link opening and pills`.

### Task 11: Composer with markdown, mention and emoji autocomplete, typing notices (roadmap step 6, UI)
Language: rust
Files: crates/app/src/composer.rs, crates/app/src/emoji.rs, crates/app/src/lib.rs, crates/app/src/state.rs, crates/app/src/screens/main_view.rs, crates/app/tests/bridge.rs, PROGRESS.md
Depends on: Task 10
Acceptance criteria:
- `App.composer` becomes `text_editor::Content`, wrapped in a `Composer` struct with `mentions: Vec<MentionRef>`, `mode: Normal | Reply(item_id) | Edit(item_id)` and `suggestions`.
  - Its `Debug` prints only the length and the mode.
  - `Message::Composer(EditorAction)` has a redacted `Debug`, unit-tested with a pasted `secret-body-x`. The Phase 2 redaction test is extended to the editor.
- Key bindings per Q17 come from a pure `bind(&KeyPress-like input, suggestions_open) -> Decision`, unit-tested: Enter sends, Shift+Enter adds a newline, Enter or Tab with suggestions picks, Up and Down move, Escape closes or cancels.
- Sending builds `Command::Send(OutgoingMessage)` through a pure `outgoing(&Composer, room_id)`:
  - trims trailing blank lines and refuses blank text;
  - `/me` gives `Emote`;
  - keeps only mentions whose label is still present;
  - fills `reply_to` in Reply mode; Edit mode sends `Command::Edit`.

  After sending, the editor and mode reset.
- Autocomplete per Q18:
  - pure `token_before_cursor(text, cursor) -> Option<Token::{Mention(q), Emoji(q)}>`, unit-tested at line start, mid-word (none), after punctuation, multi-line and multi-byte;
  - mention queries send `SearchMembers` (rate-limited, unit test with a scripted clock), and `Members` replies fill the list;
  - `emoji::suggest(q)` returns at most 8 by shortcode prefix (`smi` includes 😄 `smile`), and `emoji::exact(":smile:")` gives 😄;
  - picking replaces the token and records a `MentionRef` for members.
- Typing per Q13: `SetTyping(true)` at most every 3 s while non-empty (scripted-clock unit test), and `SetTyping(false)` on clear, send and deselect.
- `tests/bridge.rs` gains a headless test (verbatim ignore reason): through `update` only, alice selects a room, types `**hi**` and presses Enter. Her applied timeline shows a `Message` whose `RichText` has bold `hi` within 30 s.
- Human checklist gains:
  - Enter, Shift+Enter, multi-line paste, and `@` / `:` completion with keyboard and mouse;
  - **IME input** (for example ibus/fcitx Japanese or Chinese, macOS and Windows IMEs): composing doesn't send on Enter, and committed text appears;
  - typing shows on another client.
- Gate and the ignored run pass. Commit: `phase3.11: text editor composer with markdown, autocomplete and typing notices`.

### Task 12: Message actions: reply, edit, redact, react with emoji picker, copy (roadmap step 7, UI)
Language: rust
Files: crates/app/src/actions.rs, crates/app/src/emoji.rs, crates/app/src/screens/timeline_pane.rs, crates/app/src/screens/main_view.rs, crates/app/src/state.rs, PROGRESS.md
Depends on: Task 11
Acceptance criteria:
- Hovering a message (`mouse_area` `on_enter`/`on_exit`, one hovered ID in state) shows an action bar with buttons enabled from `details.actions`: Reply, Edit (own only), React, Copy and Delete.
- Reply mode:
  - **Reply** sets `mode = Reply(id)` and shows a quote bar above the composer (sanitized sender and the first line of the body) with ✕;
  - Escape or ✕ clears it.
- Edit mode:
  - **Edit** loads the message's plain `body` into the editor with `mode = Edit(id)`, and the send path gives `Command::Edit`;
  - Escape restores the previous draft (kept in memory, not logged).
- **Delete** asks for confirmation inline, then sends `Redact { reason: None }`.
- **Copy** does `clipboard::write(body)` as a `Task`.
- **React** opens the picker (Q18) in a `float` overlay. A pick sends `ToggleReaction`.
- The reactions row under a message shows `key count` chips, highlighted when `own`. Clicking a chip toggles it, and a "+" chip opens the picker.
- `ActionFailed` shows a non-secret, one-line notice for 5 s in the room pane.
- Pure helpers, unit-tested:
  - `action_buttons(&MessageDetails, is_own)`;
  - the `quote_line` cap;
  - `picker_results(query)` (at most 64; empty query gives the quick row plus group 1);
  - every action's `update` path to the right `Command` (with a plain `mpsc` sink).
- Human checklist gains: hover actions appear and disappear cleanly; reply, edit, delete and react round-trip with Element on the real account; copy puts the text on the clipboard; the picker is usable by keyboard and mouse.
- Gate passes. Commit: `phase3.12: message actions with reply, edit, redact, reactions picker and copy`.

### Task 13: Read receipts, typing display, local echo states; Phase 3 wrap-up (roadmap steps 8 and 9, UI)
Language: rust
Files: crates/app/src/screens/timeline_pane.rs, crates/app/src/screens/main_view.rs, crates/app/src/state.rs, crates/app/src/timeline.rs, PROGRESS.md
Depends on: Task 12
Acceptance criteria:
- Receipts per Q12:
  - `read_by` renders as "Seen by A, B, C +N" (sanitized short names: display name from the timeline's known senders, else the localpart);
  - `MarkRead { fully_read: false }` follows the visibility, debounce and change rule (scripted unit tests: no send while scrolled up, one send per new bottom item, none for the same item);
  - `MarkRead { fully_read: true }` is sent on deselect after the bottom was seen.
- Typing display per Q13, with the sanitized and capped names; `typing_line(&[TypingUser])` is unit-tested for 0, 1, 2 and 3+ users.
- Local echo per Q11:
  - `Sending` items render muted with a clock mark;
  - `Failed` items render in the danger colour with **Retry** (`RetrySend`) and **Discard** (`DiscardSend`), plus "Will retry when back online" when `recoverable`;
  - unit tests on the row model and on the commands sent.
- The Task 8 timing test and the Task 9 layout test still pass, with receipts and typing included.
- PROGRESS.md:
  - marks the Phase 3 agent work done; the human checkpoint stays open;
  - consolidates the open issues: editor buffer not zeroized (Q17), the residual body-styling spoof (Q20), raw HTML passthrough on send (Q10), the Phase 2 `text_safety` ranges (hand-written by the human's decision; their known gaps now also apply to message bodies), and the scope cuts (Q24).
- Gate and the ignored run (both packages) pass. Commit: `phase3.13: read receipts, typing and local echo states in the ui`.

### Phase 3 human checkpoint (collected in PROGRESS.md)
- [x] Phase 2 signed off by the human on 2026-10-02 (merge of PR #3); additive API approved as the baseline; Unicode crate rejected (Q1).
- [ ] Approve Q2 (dependencies) and Q3 (RUSTSEC-2026-0319 ignore), and revisit Q1-Q25.
- [ ] CI green on Linux, Windows and macOS with the new dependencies; the `integration` job stays under its 20-minute timeout.
- [ ] **A day of real unencrypted chat** on the real account, with Element on the other side.
- [ ] Scrolling: smooth in a busy room and in the seeded 10,000-event local room; no blank gaps when dragging the scrollbar; no jumps while new messages arrive and you're reading history.
- [ ] Scroll position during back-pagination: the message under your eyes stays put while older messages load; "Beginning of the room" appears at the start.
- [ ] Composer keys: Enter sends, Shift+Enter adds a line, Escape cancels reply and edit, completion works with keyboard and mouse.
- [ ] IME input (if you use it): composition doesn't send on Enter, and committed text is correct on each OS.
- [ ] Formatting from Element renders correctly; links open only after the expected checks; pills and spoilers behave.
- [ ] Reply, edit, delete and react round-trip with Element; the reactions picker works; copy works.
- [ ] Read receipts and the "New messages" marker look right; Element shows our receipts; typing shows both ways.
- [ ] Pending and failed messages: unplug the network, send, see pending, reconnect, see it send; a failed message can be retried or discarded.
- [ ] Day separators and times are in local time and correct around midnight.
