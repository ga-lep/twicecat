//! HTML formatted bodies to SDK-free rich text (Phase 3 Q4, Q5, Q6, Q19).
//!
//! The raw event JSON is preferred over the SDK's sanitized body so that
//! `script` and `style` can be dropped together with their contents. Parsing
//! is infallible; anything outside the allow-list is unwrapped or dropped.
//! Nothing here logs: bodies and URLs are untrusted content.

use std::mem;

use matrix_sdk::ruma::html::{Html, NodeData, NodeRef};
use matrix_sdk::ruma::matrix_uri::MatrixId;
use matrix_sdk::ruma::{MatrixToUri, MatrixUri};
use serde_json::Value;
use url::Url;

use crate::api::{Block, Inline, MentionTarget, RichText, TextStyle, UserId};

/// Elements nested deeper than this are flattened to their text.
const MAX_DEPTH: usize = 32;
/// Inlines per message.
const MAX_INLINES: usize = 2_000;
/// Characters of text per message; more is cut and marked with an ellipsis.
const MAX_TEXT_CHARS: usize = 16_384;
/// List items per message.
const MAX_LIST_ITEMS: usize = 1_000;
/// Paragraphs, headings, code blocks and rules per message.
const MAX_BLOCKS: usize = 1_000;
/// Longest link target kept, before and after normalization.
const MAX_URL_BYTES: usize = 2_048;
/// Start-tag candidates in the markup above which it is not parsed at all.
/// This bounds the parsed tree's depth, and dropping a deep tree recurses.
const MAX_TAGS: usize = 4_000;
/// Bytes a token may exceed `MAX_URL_BYTES` by (leading and trailing
/// punctuation) and still be tried as a link; longer tokens never are.
const MAX_TOKEN_MARGIN: usize = 64;
/// The only `format` that is parsed.
const HTML_FORMAT: &str = "org.matrix.custom.html";

/// Elements dropped together with their contents.
const DROPPED: &[&str] = &[
    "script", "style", "head", "title", "template", "iframe", "object", "embed", "noscript", "svg",
    "math", "mx-reply",
];

/// Elements that separate text when only their text is collected.
const BLOCKISH: &[&str] = &[
    "p",
    "div",
    "li",
    "ul",
    "ol",
    "blockquote",
    "pre",
    "tr",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
];

/// Rich text for a message: the formatted body when there is a usable one,
/// else the plain `body`.
///
/// `raw_json` is the latest raw event (an edit when there is one) and
/// `sdk_html` the SDK's sanitized formatted body, used only without raw JSON.
pub(crate) fn message_rich_text(
    raw_json: Option<&str>,
    sdk_html: Option<&str>,
    body: &str,
) -> RichText {
    let raw: Option<Value> = raw_json.and_then(|json| serde_json::from_str(json).ok());
    match select_html(raw.as_ref(), sdk_html) {
        Some(html) => from_html(&html, body),
        None => from_plain(body),
    }
}

/// The HTML to map: the formatted body of the raw event (of its
/// `m.new_content` for an edit) when its `format` is HTML. Without raw JSON,
/// the SDK's body.
pub(crate) fn select_html(raw: Option<&Value>, sdk_html: Option<&str>) -> Option<String> {
    match raw {
        Some(raw) => raw_html(raw),
        None => sdk_html.map(str::to_owned),
    }
}

fn raw_html(raw: &Value) -> Option<String> {
    let content = raw.get("content")?;
    let is_edit = content
        .get("m.relates_to")
        .and_then(|r| r.get("rel_type"))
        .and_then(Value::as_str)
        == Some("m.replace");
    let content = if is_edit {
        content
            .get("m.new_content")
            .filter(|v| v.is_object())
            .unwrap_or(content)
    } else {
        content
    };
    if content.get("format")?.as_str()? != HTML_FORMAT {
        return None;
    }
    content.get("formatted_body")?.as_str().map(str::to_owned)
}

/// Maps an HTML fragment to rich text. A fragment that maps to no visible
/// text gives the linkified plain `body` instead.
pub(crate) fn from_html(html: &str, body: &str) -> RichText {
    if too_many_tags(html, MAX_TAGS) {
        return from_plain(body);
    }
    let dom = ParsedHtml::parse(html);
    let mut builder = Builder::new();
    for node in dom.children() {
        builder.walk(&node, TextStyle::default(), 1);
    }
    let mapped = builder.finish();
    if has_text(&mapped.blocks) {
        mapped
    } else {
        from_plain(body)
    }
}

/// A parsed HTML tree that is torn down without recursion.
///
/// `ruma-html`'s tree drops recursively, one stack frame set per level, and
/// the parser adds implicit elements (`<table><td>` also opens a `tbody` and
/// a `tr`), so a tree within the tag cap can still be deep enough to
/// overflow a 2 MiB stack when dropped. The nodes are collected breadth-first
/// instead, the tree is dropped (its top level only), and then the nodes are
/// dropped parents first, so each drop frees one node whose children are
/// still held by the list.
pub(crate) struct ParsedHtml(Option<Html>);

impl ParsedHtml {
    /// Parses `html` (infallible).
    pub(crate) fn parse(html: &str) -> Self {
        Self(Some(Html::parse(html)))
    }
}

impl std::ops::Deref for ParsedHtml {
    type Target = Html;

    fn deref(&self) -> &Html {
        self.0.as_ref().expect("the tree is only taken on drop")
    }
}

impl Drop for ParsedHtml {
    fn drop(&mut self) {
        let Some(html) = self.0.take() else {
            return;
        };
        let mut nodes: Vec<NodeRef> = html.children().collect();
        let mut next = 0;
        while let Some(node) = nodes.get(next) {
            let children: Vec<NodeRef> = node.children().collect();
            nodes.extend(children);
            next += 1;
        }
        drop(html);
        // Front to back: a parent goes before its children.
        drop(nodes);
    }
}

/// Maps a plain body to rich text: one paragraph per line, URLs linkified.
pub(crate) fn from_plain(body: &str) -> RichText {
    if body.is_empty() {
        return RichText::default();
    }
    let mut builder = Builder::new();
    for line in body.split('\n') {
        if builder.done {
            break;
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        builder.text_keep(line, TextStyle::default());
        builder.flush(false);
        if line.is_empty() {
            builder.leaf(Block::Paragraph(Vec::new()));
        }
    }
    builder.finish()
}

/// Whether `html` has more than `limit` start-tag candidates (`<` followed by
/// an ASCII letter). The parsed tree is never deeper than the number of
/// elements it contains, and an element needs such a candidate, so this bounds
/// the depth without mirroring the tokenizer (which ignores `</>`, turns
/// `</ x>` into comments, and so on). It over-counts closed elements, void
/// elements and comments, which is the safe side.
fn too_many_tags(html: &str, limit: usize) -> bool {
    html.as_bytes()
        .windows(2)
        .filter(|w| w[0] == b'<' && w[1].is_ascii_alphabetic())
        .nth(limit)
        .is_some()
}

fn has_text(blocks: &[Block]) -> bool {
    fn inlines_have_text(inlines: &[Inline]) -> bool {
        inlines.iter().any(|i| match i {
            Inline::Text { text, .. } | Inline::Link { text, .. } => !text.trim().is_empty(),
            Inline::Mention { label, .. } => !label.trim().is_empty(),
            Inline::Spoiler(inner) => inlines_have_text(inner),
            Inline::LineBreak => false,
        })
    }
    blocks.iter().any(|b| match b {
        Block::Paragraph(i) | Block::Heading { inlines: i, .. } => inlines_have_text(i),
        Block::Quote(inner) => has_text(inner),
        Block::List { items, .. } => items.iter().any(|item| has_text(item)),
        Block::Code { text } => !text.trim().is_empty(),
        Block::Rule => false,
    })
}

/// Normalizes a link target, or `None` when the core does not allow it:
/// only `http`, `https` and `mailto`, no userinfo, at most 2,048 bytes.
fn normalize_url(href: &str) -> Option<String> {
    if href.len() > MAX_URL_BYTES {
        return None;
    }
    let mut url = Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "mailto") {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if url.scheme() != "mailto" && url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    if url.scheme() == "mailto" {
        url = keep_safe_mailto_query(url);
    }
    let normalized = String::from(url);
    (normalized.len() <= MAX_URL_BYTES).then_some(normalized)
}

/// Drops every `mailto:` query parameter but `subject`, `body`, `cc` and
/// `bcc` (mail clients honour others, such as `attach`, to read local files).
fn keep_safe_mailto_query(mut url: Url) -> Url {
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            ["subject", "body", "cc", "bcc"]
                .iter()
                .any(|safe| k.eq_ignore_ascii_case(safe))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    if !kept.is_empty() {
        url.query_pairs_mut().extend_pairs(kept);
    }
    url
}

/// The mention a `matrix.to` or `matrix:` link stands for.
fn mention_target(href: &str) -> Option<MentionTarget> {
    let id = if let Ok(uri) = MatrixToUri::parse(href) {
        uri.id().clone()
    } else {
        MatrixUri::parse(href).ok()?.id().clone()
    };
    match id {
        MatrixId::User(user) => Some(MentionTarget::User(UserId::from(user.to_string()))),
        MatrixId::Room(room) => Some(MentionTarget::Room(room.to_string())),
        MatrixId::RoomAlias(alias) => Some(MentionTarget::Room(alias.to_string())),
        MatrixId::Event(room, _) => Some(MentionTarget::Room(room.to_string())),
        _ => None,
    }
}

/// `#rrggbb` (either case) as red, green, blue; anything else is `None`.
fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// The label of a mention with no text of its own: the user ID, or the room
/// ID or alias (the app shows a known room's name instead).
fn id_label(target: &MentionTarget) -> String {
    match target {
        MentionTarget::User(user) => user.as_str().to_owned(),
        MentionTarget::Room(room) => room.clone(),
    }
}

// ---- linkify ---------------------------------------------------------------

enum Piece<'a> {
    Text(&'a str),
    Url { text: &'a str, url: String },
}

/// Splits `text` into plain pieces and `http(s)` URLs (Q6): a URL token ends
/// at whitespace, minus trailing `.,;:!?'">` and unbalanced `)`.
fn linkify(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut plain_from = 0;
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let end = text[i..]
            .find(char::is_whitespace)
            .map_or(text.len(), |n| i + n);
        if let Some((lead, url_text, url)) = url_in_token(&text[i..end]) {
            let start = i + lead;
            if plain_from < start {
                out.push(Piece::Text(&text[plain_from..start]));
            }
            out.push(Piece::Url {
                text: url_text,
                url,
            });
            plain_from = start + url_text.len();
        }
        i = end;
    }
    if plain_from < text.len() {
        out.push(Piece::Text(&text[plain_from..]));
    }
    out
}

/// The URL inside one whitespace-free token: the length of the leading
/// characters before it, its text, and its normalized form.
fn url_in_token(token: &str) -> Option<(usize, &str, String)> {
    if token.len() > MAX_URL_BYTES + MAX_TOKEN_MARGIN {
        return None;
    }
    let lead = token.len() - token.trim_start_matches(['(', '<', '\'', '"']).len();
    let rest = &token[lead..];
    let has_scheme = |scheme: &str| {
        rest.get(..scheme.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
    };
    if !has_scheme("http://") && !has_scheme("https://") {
        return None;
    }
    let mut end = rest.len();
    // Counted once and updated while stripping, so the loop is linear.
    let opens = rest.matches('(').count();
    let mut closes = rest.matches(')').count();
    while let Some(c) = rest[..end].chars().next_back() {
        let strip = match c {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '>' => true,
            ')' if closes > opens => {
                closes -= 1;
                true
            }
            _ => false,
        };
        if !strip {
            break;
        }
        end -= c.len_utf8();
    }
    let candidate = &rest[..end];
    let url = normalize_url(candidate)?;
    Some((lead, candidate, url))
}

// ---- text helpers ----------------------------------------------------------

/// Collapses HTML whitespace runs to a single space.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

fn local_name(node: &NodeRef) -> Option<&str> {
    match node.data() {
        NodeData::Element(el) => Some(&el.name.local),
        _ => None,
    }
}

fn attr(el: &matrix_sdk::ruma::html::ElementData, name: &str) -> Option<String> {
    el.attrs
        .borrow()
        .iter()
        .find(|a| &*a.name.local == name)
        .map(|a| a.value.to_string())
}

/// The text of `root`'s descendants, without recursion. Dropped elements are
/// skipped, `br` and block elements separate. Stops once it is clearly past
/// the text budget.
fn collect_text(root: &NodeRef, preserve: bool) -> String {
    let separator = if preserve { '\n' } else { ' ' };
    let limit = MAX_TEXT_CHARS * 4;
    let mut out = String::new();
    let mut stack: Vec<NodeRef> = root.children().collect();
    stack.reverse();
    while let Some(node) = stack.pop() {
        if out.len() > limit {
            break;
        }
        match node.data() {
            NodeData::Text(text) => out.push_str(&text.borrow()),
            NodeData::Element(el) => {
                let name: &str = &el.name.local;
                if DROPPED.contains(&name) {
                    continue;
                }
                if name == "br" {
                    out.push(separator);
                    continue;
                }
                if BLOCKISH.contains(&name) && !out.is_empty() && !out.ends_with(separator) {
                    out.push(separator);
                }
                let mut children: Vec<NodeRef> = node.children().collect();
                children.reverse();
                stack.extend(children);
            }
            _ => {}
        }
    }
    out
}

/// Removes leading and trailing blanks and line breaks.
fn trim_inlines(inlines: &mut Vec<Inline>) {
    while let Some(first) = inlines.first_mut() {
        match first {
            Inline::Text { text, .. } => {
                let trimmed = text.trim_start();
                if trimmed.len() != text.len() {
                    *text = trimmed.to_owned();
                }
                if !text.is_empty() {
                    break;
                }
            }
            Inline::LineBreak => {}
            _ => break,
        }
        inlines.remove(0);
    }
    while let Some(last) = inlines.last_mut() {
        match last {
            Inline::Text { text, .. } => {
                text.truncate(text.trim_end().len());
                if !text.is_empty() {
                    break;
                }
            }
            Inline::LineBreak => {}
            _ => break,
        }
        inlines.pop();
    }
}

// ---- the builder -----------------------------------------------------------

#[derive(Default)]
struct Frame {
    blocks: Vec<Block>,
    inlines: Vec<Inline>,
}

/// Accumulates blocks while the tree is walked, enforcing the caps.
struct Builder {
    root: Frame,
    stack: Vec<Frame>,
    inlines_left: usize,
    chars_left: usize,
    items_left: usize,
    blocks_left: usize,
    /// Inside a spoiler only inline elements are honoured.
    spoiler: usize,
    /// A cap was hit: the rest of the input is ignored.
    done: bool,
}

impl Builder {
    fn new() -> Self {
        Self {
            root: Frame::default(),
            stack: Vec::new(),
            inlines_left: MAX_INLINES,
            chars_left: MAX_TEXT_CHARS,
            items_left: MAX_LIST_ITEMS,
            blocks_left: MAX_BLOCKS,
            spoiler: 0,
            done: false,
        }
    }

    fn top(&mut self) -> &mut Frame {
        self.stack.last_mut().unwrap_or(&mut self.root)
    }

    fn finish(mut self) -> RichText {
        self.flush(true);
        RichText {
            blocks: self.root.blocks,
        }
    }

    /// Ends the current paragraph.
    fn flush(&mut self, trim: bool) {
        let mut inlines = mem::take(&mut self.top().inlines);
        if trim {
            trim_inlines(&mut inlines);
        }
        if !inlines.is_empty() {
            self.leaf(Block::Paragraph(inlines));
        }
    }

    /// Adds a leaf block, within the block cap.
    fn leaf(&mut self, block: Block) {
        if self.blocks_left == 0 {
            self.done = true;
            return;
        }
        self.blocks_left -= 1;
        self.top().blocks.push(block);
    }

    fn enter(&mut self) {
        self.stack.push(Frame::default());
    }

    fn exit(&mut self) -> Vec<Block> {
        self.flush(true);
        self.stack.pop().map(|f| f.blocks).unwrap_or_default()
    }

    /// Takes up to the remaining character budget from `s`. The flag tells
    /// that text was cut, in which case the caller adds an ellipsis.
    fn budget<'a>(&mut self, s: &'a str) -> (&'a str, bool) {
        match s.char_indices().nth(self.chars_left) {
            None => {
                self.chars_left -= s.chars().count();
                (s, false)
            }
            Some((cut, _)) => {
                self.chars_left = 0;
                self.done = true;
                (&s[..cut], true)
            }
        }
    }

    fn inline_room(&mut self) -> bool {
        if self.inlines_left == 0 {
            self.done = true;
            return false;
        }
        self.inlines_left -= 1;
        true
    }

    fn push_inline(&mut self, inline: Inline) {
        if self.inline_room() {
            self.top().inlines.push(inline);
        }
    }

    /// Text from a document: a blank directly after a blank (or at the start
    /// of a line) is dropped, URLs are linkified outside inline code.
    fn text(&mut self, s: &str, style: TextStyle) {
        let after_blank = match self.top().inlines.last() {
            None | Some(Inline::LineBreak) => true,
            Some(Inline::Text { text, .. }) => text.ends_with(' '),
            Some(_) => false,
        };
        let s = if after_blank {
            s.strip_prefix(' ').unwrap_or(s)
        } else {
            s
        };
        self.text_keep(s, style);
    }

    fn text_keep(&mut self, s: &str, style: TextStyle) {
        if s.is_empty() || self.done {
            return;
        }
        if style.code {
            self.plain(s, style);
            return;
        }
        for piece in linkify(s) {
            match piece {
                Piece::Text(text) => self.plain(text, style),
                // A pasted permalink (Element's "Copy room link") is a pill,
                // as Element shows it, not a web link.
                Piece::Url { text, url } => match mention_target(text) {
                    Some(target) => {
                        let label = id_label(&target);
                        self.mention(target, &label);
                    }
                    None => self.link(text, url, style),
                },
            }
        }
    }

    fn plain(&mut self, s: &str, style: TextStyle) {
        if self.done {
            return;
        }
        let (kept, cut) = self.budget(s);
        let mut text = kept.to_owned();
        if cut {
            text.push('…');
        }
        if text.is_empty() {
            return;
        }
        if let Some(Inline::Text {
            text: last,
            style: last_style,
        }) = self.top().inlines.last_mut()
            && *last_style == style
        {
            last.push_str(&text);
            return;
        }
        self.push_inline(Inline::Text { text, style });
    }

    fn link(&mut self, text: &str, url: String, style: TextStyle) {
        if self.done {
            return;
        }
        let (kept, cut) = self.budget(text);
        let mut text = kept.to_owned();
        if cut {
            text.push('…');
        }
        self.push_inline(Inline::Link { text, url, style });
    }

    fn mention(&mut self, target: MentionTarget, label: &str) {
        if self.done {
            return;
        }
        let (kept, cut) = self.budget(label);
        let mut label = kept.to_owned();
        if cut {
            label.push('…');
        }
        self.push_inline(Inline::Mention { target, label });
    }

    fn walk_children(&mut self, node: &NodeRef, style: TextStyle, depth: usize) {
        for child in node.children() {
            if self.done {
                break;
            }
            self.walk(&child, style, depth + 1);
        }
    }

    fn walk(&mut self, node: &NodeRef, style: TextStyle, depth: usize) {
        if self.done {
            return;
        }
        match node.data() {
            NodeData::Text(text) => {
                let collapsed = collapse_ws(&text.borrow());
                self.text(&collapsed, style);
            }
            NodeData::Element(el) => self.element(node, el, style, depth),
            _ => {}
        }
    }

    fn element(
        &mut self,
        node: &NodeRef,
        el: &matrix_sdk::ruma::html::ElementData,
        style: TextStyle,
        depth: usize,
    ) {
        let name: &str = &el.name.local;
        if DROPPED.contains(&name) {
            return;
        }
        if depth > MAX_DEPTH {
            let flat = collapse_ws(&collect_text(node, false));
            self.text(&flat, style);
            return;
        }
        let blocks_ok = self.spoiler == 0;
        match name {
            "b" | "strong" => {
                self.walk_children(
                    node,
                    TextStyle {
                        bold: true,
                        ..style
                    },
                    depth,
                );
            }
            "i" | "em" => {
                self.walk_children(
                    node,
                    TextStyle {
                        italic: true,
                        ..style
                    },
                    depth,
                );
            }
            "del" | "s" | "strike" => {
                self.walk_children(
                    node,
                    TextStyle {
                        strike: true,
                        ..style
                    },
                    depth,
                );
            }
            "u" => {
                self.walk_children(
                    node,
                    TextStyle {
                        underline: true,
                        ..style
                    },
                    depth,
                );
            }
            "code" => {
                self.walk_children(
                    node,
                    TextStyle {
                        code: true,
                        ..style
                    },
                    depth,
                );
            }
            "a" => self.anchor(node, el, style, depth),
            "br" => self.push_inline(Inline::LineBreak),
            "img" => {
                if let Some(alt) = attr(el, "alt") {
                    self.text(&collapse_ws(&alt), style);
                }
            }
            "span" if blocks_ok && attr(el, "data-mx-spoiler").is_some() => {
                self.spoiler(node, style, depth);
            }
            // Text colour, as Element sends it (`/rainbow`, the colour
            // picker): `data-mx-color` on `span` or `font`, or the older
            // `font color`.
            "span" | "font" => {
                let color = attr(el, "data-mx-color")
                    .or_else(|| (name == "font").then(|| attr(el, "color")).flatten())
                    .and_then(|value| parse_hex_color(&value));
                self.walk_children(
                    node,
                    TextStyle {
                        color: color.or(style.color),
                        ..style
                    },
                    depth,
                );
            }
            "pre" if blocks_ok => {
                self.flush(true);
                let mut code = collect_text(node, true);
                if code.ends_with('\n') {
                    code.pop();
                }
                if !code.trim().is_empty() {
                    let (kept, cut) = self.budget(&code);
                    let mut text = kept.to_owned();
                    if cut {
                        text.push('…');
                    }
                    self.leaf(Block::Code { text });
                }
            }
            "hr" if blocks_ok => {
                self.flush(true);
                self.leaf(Block::Rule);
            }
            "blockquote" if blocks_ok => {
                self.flush(true);
                self.enter();
                self.walk_children(node, style, depth);
                let inner = self.exit();
                if !inner.is_empty() {
                    self.top().blocks.push(Block::Quote(inner));
                }
            }
            "ul" | "ol" if blocks_ok => self.list(node, el, style, depth),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if blocks_ok => {
                self.flush(true);
                self.walk_children(node, style, depth);
                let mut inlines = mem::take(&mut self.top().inlines);
                trim_inlines(&mut inlines);
                if !inlines.is_empty() {
                    let level = name.as_bytes()[1] - b'0';
                    self.leaf(Block::Heading { level, inlines });
                }
            }
            "tr" if blocks_ok => {
                self.flush(true);
                let mut first_cell = true;
                for child in node.children() {
                    if self.done {
                        break;
                    }
                    if matches!(local_name(&child), Some("td" | "th")) {
                        if !first_cell {
                            self.text_keep("  ", style);
                        }
                        first_cell = false;
                    }
                    self.walk(&child, style, depth + 1);
                }
                self.flush(true);
            }
            "p" | "div" | "li" | "table" | "thead" | "tbody" | "tfoot" if blocks_ok => {
                self.flush(true);
                self.walk_children(node, style, depth);
                self.flush(true);
            }
            _ => self.walk_children(node, style, depth),
        }
    }

    fn anchor(
        &mut self,
        node: &NodeRef,
        el: &matrix_sdk::ruma::html::ElementData,
        style: TextStyle,
        depth: usize,
    ) {
        if let Some(href) = attr(el, "href").filter(|h| h.len() <= MAX_URL_BYTES) {
            let label = || collapse_ws(&collect_text(node, false)).trim().to_owned();
            if let Some(target) = mention_target(&href) {
                let mut label = label();
                // No text, or the bare permalink: show the ID instead.
                if label.is_empty() || label == href {
                    label = id_label(&target);
                }
                self.mention(target, &label);
                return;
            }
            if let Some(url) = normalize_url(&href) {
                let mut label = label();
                if label.is_empty() {
                    label.clone_from(&url);
                }
                self.link(&label, url, style);
                return;
            }
        }
        self.walk_children(node, style, depth);
    }

    fn spoiler(&mut self, node: &NodeRef, style: TextStyle, depth: usize) {
        let saved = mem::take(&mut self.top().inlines);
        self.spoiler += 1;
        self.walk_children(node, style, depth);
        self.spoiler -= 1;
        let mut inner = mem::replace(&mut self.top().inlines, saved);
        trim_inlines(&mut inner);
        if !inner.is_empty() {
            self.push_inline(Inline::Spoiler(inner));
        }
    }

    fn list(
        &mut self,
        node: &NodeRef,
        el: &matrix_sdk::ruma::html::ElementData,
        style: TextStyle,
        depth: usize,
    ) {
        self.flush(true);
        let ordered = &*el.name.local == "ol";
        let start = if ordered {
            attr(el, "start")
                .and_then(|s| s.trim().parse::<u64>().ok())
                .filter(|&n| n != 1)
        } else {
            None
        };
        let mut items = Vec::new();
        for child in node.children() {
            if self.done || self.items_left == 0 {
                break;
            }
            let is_li = local_name(&child) == Some("li");
            let ignorable = match child.data() {
                NodeData::Text(text) => text.borrow().trim().is_empty(),
                NodeData::Element(_) => false,
                _ => true,
            };
            if ignorable {
                continue;
            }
            self.enter();
            if is_li {
                self.walk_children(&child, style, depth + 1);
            } else {
                self.walk(&child, style, depth + 1);
            }
            let blocks = self.exit();
            if !blocks.is_empty() {
                self.items_left -= 1;
                items.push(blocks);
            }
        }
        if !items.is_empty() {
            self.top().blocks.push(Block::List {
                ordered,
                start,
                items,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::api::{Block, Inline, MentionTarget, TextStyle, UserId};

    // ---- helpers -------------------------------------------------------

    fn dump_inline(i: &Inline) -> String {
        match i {
            Inline::Text { text, style } => format!("T{style:?}{text:?}"),
            Inline::Link { text, url, style } => format!("L{style:?}{text:?}->{url:?}"),
            Inline::Mention { target, label } => format!("M{target:?}{label:?}"),
            Inline::LineBreak => "BR".to_owned(),
            Inline::Spoiler(v) => format!("S[{}]", dump_inlines(v)),
        }
    }

    fn dump_inlines(v: &[Inline]) -> String {
        v.iter().map(dump_inline).collect::<Vec<_>>().join(",")
    }

    fn dump_block(b: &Block) -> String {
        match b {
            Block::Paragraph(i) => format!("P[{}]", dump_inlines(i)),
            Block::Heading { level, inlines } => format!("H{level}[{}]", dump_inlines(inlines)),
            Block::Quote(b) => format!("Q[{}]", dump_blocks(b)),
            Block::List {
                ordered,
                start,
                items,
            } => format!(
                "List{{{ordered},{start:?},[{}]}}",
                items
                    .iter()
                    .map(|i| format!("({})", dump_blocks(i)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Block::Code { text } => format!("Code{text:?}"),
            Block::Rule => "Rule".to_owned(),
        }
    }

    fn dump_blocks(b: &[Block]) -> String {
        b.iter().map(dump_block).collect::<Vec<_>>().join(",")
    }

    /// `RichText`'s own `Debug` hides content, so compare readable dumps.
    fn assert_rt(actual: &RichText, expected: &[Block]) {
        assert_eq!(dump_blocks(&actual.blocks), dump_blocks(expected));
        assert_eq!(actual.blocks, expected);
    }

    fn style(f: impl FnOnce(&mut TextStyle)) -> TextStyle {
        let mut s = TextStyle::default();
        f(&mut s);
        s
    }

    fn t(text: &str) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            style: TextStyle::default(),
        }
    }

    fn ts(text: &str, style: TextStyle) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            style,
        }
    }

    fn link(text: &str, url: &str) -> Inline {
        Inline::Link {
            text: text.to_owned(),
            url: url.to_owned(),
            style: TextStyle::default(),
        }
    }

    fn p(inlines: Vec<Inline>) -> Block {
        Block::Paragraph(inlines)
    }

    fn first_inlines(rt: &RichText) -> &[Inline] {
        match rt.blocks.first() {
            Some(Block::Paragraph(i)) => i,
            other => panic!(
                "expected a paragraph, got {}",
                other.map_or(String::new(), dump_block)
            ),
        }
    }

    fn all_inlines<'a>(blocks: &'a [Block], out: &mut Vec<&'a Inline>) {
        for b in blocks {
            match b {
                Block::Paragraph(i) | Block::Heading { inlines: i, .. } => {
                    for x in i {
                        out.push(x);
                        if let Inline::Spoiler(inner) = x {
                            out.extend(inner.iter());
                        }
                    }
                }
                Block::Quote(b) => all_inlines(b, out),
                Block::List { items, .. } => {
                    for item in items {
                        all_inlines(item, out);
                    }
                }
                Block::Code { .. } | Block::Rule => {}
            }
        }
    }

    fn inline_count(rt: &RichText) -> usize {
        let mut v = Vec::new();
        all_inlines(&rt.blocks, &mut v);
        v.len()
    }

    fn all_text(rt: &RichText) -> String {
        let mut v = Vec::new();
        all_inlines(&rt.blocks, &mut v);
        let mut s: String = v
            .iter()
            .map(|i| match i {
                Inline::Text { text, .. } | Inline::Link { text, .. } => text.as_str(),
                Inline::Mention { label, .. } => label.as_str(),
                Inline::LineBreak | Inline::Spoiler(_) => "",
            })
            .collect();
        fn code(blocks: &[Block], s: &mut String) {
            for b in blocks {
                match b {
                    Block::Code { text } => s.push_str(text),
                    Block::Quote(b) => code(b, s),
                    Block::List { items, .. } => items.iter().for_each(|i| code(i, s)),
                    _ => {}
                }
            }
        }
        code(&rt.blocks, &mut s);
        s
    }

    fn quote_depth(blocks: &[Block]) -> usize {
        blocks
            .iter()
            .map(|b| match b {
                Block::Quote(inner) => 1 + quote_depth(inner),
                Block::List { items, .. } => {
                    items.iter().map(|i| quote_depth(i)).max().unwrap_or(0)
                }
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    // ---- fixtures ------------------------------------------------------

    #[test]
    fn inline_styles() {
        let rt = from_html(
            include_str!("../tests/fixtures/html/inline_styles.html"),
            "x",
        );
        assert_rt(
            &rt,
            &[p(vec![
                ts("bold", style(|s| s.bold = true)),
                t(" "),
                ts("it", style(|s| s.italic = true)),
                t(" "),
                ts("st", style(|s| s.strike = true)),
                t(" "),
                ts("un", style(|s| s.underline = true)),
                t(" "),
                ts("c", style(|s| s.code = true)),
                t(" "),
                ts(
                    "both",
                    style(|s| {
                        s.bold = true;
                        s.italic = true;
                    }),
                ),
            ])],
        );
    }

    #[test]
    fn pre_code_keeps_whitespace() {
        let rt = from_html(include_str!("../tests/fixtures/html/pre_code.html"), "x");
        assert_rt(
            &rt,
            &[
                Block::Code {
                    text: "fn main() {\n    run();\n}".to_owned(),
                },
                p(vec![t("after")]),
            ],
        );
    }

    #[test]
    fn link_is_kept() {
        let rt = from_html(include_str!("../tests/fixtures/html/link.html"), "x");
        assert_rt(
            &rt,
            &[p(vec![
                t("see "),
                link("the docs", "https://example.org/a?b=1"),
                t(" now"),
            ])],
        );
    }

    #[test]
    fn blockquote_nested_twice() {
        let rt = from_html(
            include_str!("../tests/fixtures/html/quote_nested.html"),
            "x",
        );
        assert_rt(
            &rt,
            &[Block::Quote(vec![
                p(vec![t("one")]),
                Block::Quote(vec![
                    p(vec![t("two")]),
                    Block::Quote(vec![p(vec![t("three")])]),
                ]),
            ])],
        );
    }

    #[test]
    fn lists_with_start_and_nesting() {
        let rt = from_html(include_str!("../tests/fixtures/html/lists.html"), "x");
        assert_rt(
            &rt,
            &[Block::List {
                ordered: false,
                start: None,
                items: vec![
                    vec![p(vec![t("a")])],
                    vec![
                        p(vec![t("b")]),
                        Block::List {
                            ordered: true,
                            start: Some(3),
                            items: vec![vec![p(vec![t("c")])], vec![p(vec![t("d")])]],
                        },
                    ],
                ],
            }],
        );
    }

    #[test]
    fn heading_break_and_rule() {
        let rt = from_html(
            include_str!("../tests/fixtures/html/heading_br_hr.html"),
            "x",
        );
        assert_rt(
            &rt,
            &[
                Block::Heading {
                    level: 2,
                    inlines: vec![t("Title")],
                },
                p(vec![t("line one"), Inline::LineBreak, t("line two")]),
                Block::Rule,
                p(vec![t("end")]),
            ],
        );
    }

    #[test]
    fn dropped_elements_lose_their_contents() {
        let rt = from_html(include_str!("../tests/fixtures/html/stripped.html"), "x");
        assert_rt(&rt, &[p(vec![t("keep")]), p(vec![t("also")])]);
        let text = all_text(&rt);
        for bad in ["alert", "color", "frame text", "svg text", "quoted"] {
            assert!(!text.contains(bad), "{bad} leaked");
        }
    }

    #[test]
    fn unknown_elements_are_unwrapped_and_only_matrix_colours_kept() {
        // `data-mx-color` is kept (human decision at the Phase 3 checkpoint,
        // replacing plan Q-"colours ignored"); a CSS `style` colour is not.
        let rt = from_html(include_str!("../tests/fixtures/html/unwrapped.html"), "x");
        assert_rt(
            &rt,
            &[p(vec![
                coloured("red", [0xff, 0x00, 0x00]),
                t(" unknown nested blue"),
            ])],
        );
    }

    #[test]
    fn spoiler_span() {
        let rt = from_html(r#"a <span data-mx-spoiler>secret <b>bold</b></span>"#, "x");
        assert_rt(
            &rt,
            &[p(vec![
                t("a "),
                Inline::Spoiler(vec![t("secret "), ts("bold", style(|s| s.bold = true))]),
            ])],
        );
    }

    #[test]
    fn img_alt_and_table_rows() {
        let rt = from_html(r#"<img src="x" alt="cat">"#, "x");
        assert_rt(&rt, &[p(vec![t("cat")])]);
        let rt = from_html(
            "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>",
            "x",
        );
        assert_rt(&rt, &[p(vec![t("a  b")]), p(vec![t("c  d")])]);
    }

    // ---- mentions and links -------------------------------------------

    fn anchor(href: &str, label: &str) -> RichText {
        from_html(&format!(r#"<a href="{href}">{label}</a>"#), "fallback")
    }

    fn mention(target: MentionTarget, label: &str) -> Inline {
        Inline::Mention {
            target,
            label: label.to_owned(),
        }
    }

    fn coloured(text: &str, rgb: [u8; 3]) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            style: TextStyle {
                color: Some(rgb),
                ..TextStyle::default()
            },
        }
    }

    #[test]
    fn text_colours_are_kept_like_element() {
        let rt = from_html(
            r##"a<span data-mx-color="#FF0080">b<b>c</b></span><font color="#00e371">d</font><font data-mx-color="#102030">e</font>"##,
            "abcde",
        );
        let bold_pink = Inline::Text {
            text: "c".into(),
            style: TextStyle {
                bold: true,
                color: Some([0xff, 0x00, 0x80]),
                ..TextStyle::default()
            },
        };
        assert_eq!(
            first_inlines(&rt),
            [
                t("a"),
                coloured("b", [0xff, 0x00, 0x80]),
                bold_pink,
                coloured("d", [0x00, 0xe3, 0x71]),
                coloured("e", [0x10, 0x20, 0x30]),
            ]
            .as_slice()
        );
    }

    #[test]
    fn only_six_digit_hex_colours_are_kept() {
        for bad in [
            "red",
            "#abc",
            "#12345g",
            "#1234567",
            "javascript:x",
            " #102030",
            "",
        ] {
            let rt = from_html(&format!(r#"<span data-mx-color="{bad}">x</span>"#), "x");
            assert_eq!(first_inlines(&rt), [t("x")].as_slice(), "{bad:?}");
        }
    }

    #[test]
    fn elements_rainbow_arrives_in_colour() {
        let rt = from_html(
            &crate::outgoing::rainbow_html("this is a test message"),
            "this is a test message",
        );
        let colours: std::collections::HashSet<_> = first_inlines(&rt)
            .iter()
            .filter_map(|i| match i {
                Inline::Text { style, .. } => style.color,
                _ => None,
            })
            .collect();
        assert_eq!(colours.len(), 18, "one colour per letter");
    }

    #[test]
    fn user_mentions() {
        let want = vec![mention(
            MentionTarget::User(UserId::from("@alice:localhost")),
            "Alice",
        )];
        for href in [
            "https://matrix.to/#/@alice:localhost",
            "https://matrix.to/#/%40alice%3Alocalhost",
            "matrix:u/alice:localhost",
        ] {
            assert_eq!(
                first_inlines(&anchor(href, "Alice")),
                want.as_slice(),
                "{href}"
            );
        }
    }

    #[test]
    fn room_mentions() {
        for (href, id) in [
            ("https://matrix.to/#/#room:localhost", "#room:localhost"),
            ("https://matrix.to/#/!id:localhost", "!id:localhost"),
            (
                "https://matrix.to/#/!id:localhost/$event:localhost",
                "!id:localhost",
            ),
        ] {
            let want = vec![mention(MentionTarget::Room(id.to_owned()), "R")];
            assert_eq!(first_inlines(&anchor(href, "R")), want.as_slice(), "{href}");
        }
    }

    #[test]
    fn empty_pill_label_falls_back_to_the_id() {
        let rt = anchor("https://matrix.to/#/@alice:localhost", "");
        assert_eq!(
            first_inlines(&rt),
            [mention(
                MentionTarget::User(UserId::from("@alice:localhost")),
                "@alice:localhost"
            )]
        );
    }

    #[test]
    fn disallowed_links_become_text() {
        let long = format!("https://example.org/{}", "a".repeat(2100));
        for href in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "data:text/html,x",
            "file:///etc/passwd",
            "vbscript:x",
            "ftp://example.org/",
            "https://user:pw@example.org/",
            "https://user@example.org/",
            "not a url",
            "matrix:garbage",
            long.as_str(),
        ] {
            let rt = anchor(href, "label");
            assert_rt(&rt, &[p(vec![t("label")])]);
        }
    }

    #[test]
    fn allowed_links_are_normalized() {
        let rt = anchor("HTTPS://Example.ORG/ä", "x");
        assert_rt(&rt, &[p(vec![link("x", "https://example.org/%C3%A4")])]);
        let rt = anchor("mailto:a@example.org", "mail");
        assert_rt(&rt, &[p(vec![link("mail", "mailto:a@example.org")])]);
        let rt = anchor("http://example.org", "h");
        assert_rt(&rt, &[p(vec![link("h", "http://example.org/")])]);
    }

    #[test]
    fn url_length_limit_is_2048_bytes() {
        let exact = format!("https://example.org/{}", "a".repeat(2048 - 20));
        assert_eq!(exact.len(), 2048);
        assert_eq!(normalize_url(&exact).as_deref(), Some(exact.as_str()));
        assert_eq!(normalize_url(&format!("{exact}a")), None);
    }

    // ---- fallback ------------------------------------------------------

    #[test]
    fn blank_or_unparsable_html_falls_back_to_the_linkified_body() {
        let want = [p(vec![
            t("see "),
            link("https://example.org", "https://example.org/"),
            t("."),
        ])];
        for html in [
            "",
            "   ",
            "<script>x</script>",
            "<b",
            "<p></p><hr>",
            "<style>a{}</style>",
        ] {
            let rt = from_html(html, "see https://example.org.");
            assert_rt(&rt, &want);
        }
    }

    // ---- caps ----------------------------------------------------------

    #[test]
    fn nesting_is_flattened_at_depth_32() {
        let html = format!(
            "{}deep{}",
            "<blockquote>".repeat(40),
            "</blockquote>".repeat(40)
        );
        let rt = from_html(&html, "x");
        assert_eq!(quote_depth(&rt.blocks), 32);
        assert!(all_text(&rt).contains("deep"));
    }

    #[test]
    fn inlines_are_capped_at_2000() {
        let html = "<b>a</b><i>b</i>".repeat(1500);
        let rt = from_html(&html, "x");
        assert_eq!(inline_count(&rt), 2000);
    }

    #[test]
    fn text_is_cut_with_an_ellipsis() {
        let rt = from_html(&format!("<p>{}</p>", "a".repeat(20_000)), "x");
        let text = all_text(&rt);
        assert!(text.ends_with('…'));
        assert_eq!(text.chars().count(), 16_384 + 1);
        let rt = from_html(&format!("<pre>{}</pre>", "a".repeat(20_000)), "x");
        assert!(all_text(&rt).ends_with('…'));
    }

    #[test]
    fn list_items_are_capped_at_1000() {
        let rt = from_html(&format!("<ul>{}</ul>", "<li>x</li>".repeat(1500)), "x");
        match rt.blocks.as_slice() {
            [Block::List { items, .. }] => assert_eq!(items.len(), 1000),
            _ => panic!("expected one list"),
        }
    }

    #[test]
    fn paragraphs_are_capped() {
        let body = "line\n".repeat(5000);
        assert_eq!(from_plain(&body).blocks.len(), 1000);
        let html = "<p>x</p>".repeat(3500);
        assert_eq!(from_html(&html, "x").blocks.len(), 1000);
    }

    #[test]
    fn malformed_html_does_not_panic() {
        for html in [
            "<b><i>unclosed",
            "</li></ul></div>stray",
            "a\0b<p>\0</p>",
            "<a href=\"https://example.org\">",
            "<ul><li><ul><li>",
            "<table><tr><td>",
            "<<<>>>",
            "<pre>",
            "<a>\0</a>",
        ] {
            let _ = from_html(html, "fallback");
        }
    }

    #[test]
    fn hostile_nesting_falls_back_before_parsing() {
        // The parsed tree is dropped recursively: tens of thousands of open
        // elements fit in one event and would overflow the stack.
        let want = from_plain("fallback");
        for open in ["<div>", "<b>", "<blockquote>", "<span x='<'>"] {
            let deep = format!("{}x", open.repeat(20_000));
            assert_eq!(from_html(&deep, "fallback"), want);
        }
        // Closed elements, void elements and comments do not count.
        let flat = format!("{}ok", "<b>a</b><br><!-- <i> -->".repeat(1_000));
        assert!(has_text(&from_html(&flat, "fallback").blocks));
        let ok = format!("{}ok{}", "<div>".repeat(200), "</div>".repeat(200));
        assert_ne!(from_html(&ok, "fallback"), want);
    }

    #[test]
    fn linkify_trailing_parentheses_is_linear() {
        let start = std::time::Instant::now();
        let token = format!("http://a{}", ")".repeat(65_000));
        let rt = from_plain(&token);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert!(all_text(&rt).starts_with("http://a"));
        // An over-long token is never a link.
        let long = format!("https://example.org/{}", "a".repeat(3_000));
        assert_rt(&from_plain(&long), &[p(vec![t(&long)])]);
        // Balanced and unbalanced parentheses still behave.
        assert_rt(
            &from_plain("https://e.org/(a))"),
            &[p(vec![
                link("https://e.org/(a)", "https://e.org/(a)"),
                t(")"),
            ])],
        );
    }

    #[test]
    fn nesting_guard_cannot_be_bypassed() {
        let want = from_plain("fallback");
        let cases = [
            "<b></>".repeat(10_900),
            "<b></ x>".repeat(10_900),
            "<b><!--</-->".repeat(10_900),
            "<b></>".repeat(20_000),
        ];
        for html in &cases {
            assert_eq!(from_html(html, "fallback"), want);
        }
        let deep = format!("{}x{}", "<div>".repeat(5_000), "</div>".repeat(5_000));
        assert_eq!(from_html(&deep, "fallback"), want);
        // The deepest tree that is still parsed drops without overflowing.
        for open in ["<div>", "<b>", "<blockquote>", "<ul><li>"] {
            let max = format!("{}x", open.repeat(MAX_TAGS / open.matches('<').count()));
            assert!(has_text(&from_html(&max, "fallback").blocks));
        }
        // Normal messages are untouched.
        let ok = format!("{}ok{}", "<div>".repeat(30), "</div>".repeat(30));
        assert_ne!(from_html(&ok, "fallback"), want);
    }

    /// Runs `f` on a 2 MiB thread, the smallest stack we run on.
    fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(f)
            .expect("spawn")
            .join()
            .expect("no crash on a 2 MiB stack")
    }

    #[test]
    fn implicit_table_elements_at_the_tag_cap_drop_on_a_small_stack() {
        // Each `<table><td>` also opens an implicit tbody and tr: the tree is
        // about four times as deep as the tag count.
        let html = format!("{}x", "<table><td>".repeat(MAX_TAGS / 2));
        let rt = on_small_stack(move || from_html(&html, "fallback"));
        assert!(has_text(&rt.blocks));
    }

    #[test]
    fn mailto_keeps_only_safe_query_parameters() {
        let rt = anchor(
            "mailto:a@example.org?subject=Hi&attach=/home/u/.ssh/id_rsa&body=x&bcc=b@example.org",
            "m",
        );
        assert_rt(
            &rt,
            &[p(vec![link(
                "m",
                "mailto:a@example.org?subject=Hi&body=x&bcc=b%40example.org",
            )])],
        );
        let rt = anchor("mailto:a@example.org?attach=/etc/passwd", "m");
        assert_rt(&rt, &[p(vec![link("m", "mailto:a@example.org")])]);
        let rt = anchor("mailto:a@example.org?SUBJECT=Hi", "m");
        assert_rt(
            &rt,
            &[p(vec![link("m", "mailto:a@example.org?SUBJECT=Hi")])],
        );
    }

    // ---- plain bodies and linkify -------------------------------------

    #[test]
    fn plain_body_is_one_paragraph_per_line() {
        assert!(from_plain("").blocks.is_empty());
        assert_rt(
            &from_plain("a\n\nb"),
            &[p(vec![t("a")]), p(vec![]), p(vec![t("b")])],
        );
    }

    #[test]
    fn linkify_trailing_punctuation_and_parentheses() {
        assert_rt(
            &from_plain("go to https://example.org/x."),
            &[p(vec![
                t("go to "),
                link("https://example.org/x", "https://example.org/x"),
                t("."),
            ])],
        );
        assert_rt(
            &from_plain("(https://example.org/x), really?"),
            &[p(vec![
                t("("),
                link("https://example.org/x", "https://example.org/x"),
                t("), really?"),
            ])],
        );
        assert_rt(
            &from_plain("https://en.wikipedia.org/wiki/Foo_(bar)"),
            &[p(vec![link(
                "https://en.wikipedia.org/wiki/Foo_(bar)",
                "https://en.wikipedia.org/wiki/Foo_(bar)",
            )])],
        );
        assert_rt(
            &from_plain("<https://example.org/x>"),
            &[p(vec![
                t("<"),
                link("https://example.org/x", "https://example.org/x"),
                t(">"),
            ])],
        );
    }

    #[test]
    fn linkify_two_urls_in_one_line() {
        assert_rt(
            &from_plain("https://a.example and https://b.example!"),
            &[p(vec![
                link("https://a.example", "https://a.example/"),
                t(" and "),
                link("https://b.example", "https://b.example/"),
                t("!"),
            ])],
        );
    }

    #[test]
    fn linkify_url_only_and_schemes() {
        assert_rt(
            &from_plain("https://example.org/"),
            &[p(vec![link(
                "https://example.org/",
                "https://example.org/",
            )])],
        );
        assert_rt(
            &from_plain("http://example.org"),
            &[p(vec![link("http://example.org", "http://example.org/")])],
        );
        assert_rt(
            &from_plain("ftp://example.org"),
            &[p(vec![t("ftp://example.org")])],
        );
        assert_rt(&from_plain("http://"), &[p(vec![t("http://")])]);
        assert_rt(
            &from_plain("mail a@example.org"),
            &[p(vec![t("mail a@example.org")])],
        );
    }

    #[test]
    fn a_pasted_permalink_in_plain_text_is_a_pill_like_element() {
        // Element's "Copy room link", pasted and sent as plain text.
        let room = from_plain("see https://matrix.to/#/!LuZr:localhost?via=localhost now");
        assert_eq!(
            first_inlines(&room),
            [
                t("see "),
                mention(
                    MentionTarget::Room("!LuZr:localhost".into()),
                    "!LuZr:localhost"
                ),
                t(" now"),
            ]
            .as_slice()
        );
        let alias = from_plain("https://matrix.to/#/%23lobby:localhost");
        assert_eq!(
            first_inlines(&alias),
            [mention(
                MentionTarget::Room("#lobby:localhost".into()),
                "#lobby:localhost"
            )]
            .as_slice()
        );
        let user = from_plain("ping https://matrix.to/#/@bob:localhost");
        assert_eq!(
            first_inlines(&user)[1],
            mention(
                MentionTarget::User(UserId::from("@bob:localhost")),
                "@bob:localhost"
            )
        );
        // Other URLs stay links.
        assert!(matches!(
            first_inlines(&from_plain("https://matrix.org/")),
            [Inline::Link { .. }]
        ));
    }

    #[test]
    fn an_anchor_whose_text_is_its_permalink_gets_the_id_as_label() {
        let href = "https://matrix.to/#/!LuZr:localhost?via=localhost";
        assert_eq!(
            first_inlines(&anchor(href, href)),
            [mention(
                MentionTarget::Room("!LuZr:localhost".into()),
                "!LuZr:localhost"
            )]
            .as_slice()
        );
        // A real label is kept.
        assert_eq!(
            first_inlines(&anchor(href, "Lobby")),
            [mention(
                MentionTarget::Room("!LuZr:localhost".into()),
                "Lobby"
            )]
            .as_slice()
        );
    }

    #[test]
    fn linkify_applies_to_html_text_but_not_code() {
        let rt = from_html(
            "x https://example.org <code>https://example.org</code>",
            "b",
        );
        assert_rt(
            &rt,
            &[p(vec![
                t("x "),
                link("https://example.org", "https://example.org/"),
                t(" "),
                ts("https://example.org", style(|s| s.code = true)),
            ])],
        );
    }

    // ---- source selection ---------------------------------------------

    const HTML: &str = "org.matrix.custom.html";

    #[test]
    fn source_is_the_raw_formatted_body_of_an_original_event() {
        let raw = json!({"type": "m.room.message", "content": {
            "msgtype": "m.text", "body": "b", "format": HTML, "formatted_body": "<b>x</b>"}});
        assert_eq!(
            select_html(Some(&raw), Some("<i>sdk</i>")).as_deref(),
            Some("<b>x</b>")
        );
    }

    #[test]
    fn source_of_an_edit_is_the_new_content() {
        let raw = json!({"type": "m.room.message", "content": {
            "msgtype": "m.text", "body": "* b", "format": HTML, "formatted_body": "* <i>old</i>",
            "m.new_content": {"msgtype": "m.text", "body": "b", "format": HTML,
                              "formatted_body": "<i>new</i>"},
            "m.relates_to": {"rel_type": "m.replace", "event_id": "$e"}}});
        assert_eq!(select_html(Some(&raw), None).as_deref(), Some("<i>new</i>"));
    }

    #[test]
    fn source_ignores_the_outer_body_of_an_edit_without_formatting() {
        let raw = json!({"type": "m.room.message", "content": {
            "msgtype": "m.text", "body": "* b", "format": HTML, "formatted_body": "* <i>old</i>",
            "m.new_content": {"msgtype": "m.text", "body": "b"},
            "m.relates_to": {"rel_type": "m.replace", "event_id": "$e"}}});
        assert_eq!(select_html(Some(&raw), Some("<i>sdk</i>")), None);
    }

    #[test]
    fn source_requires_the_html_format() {
        let raw = json!({"content": {"msgtype": "m.text", "body": "b",
            "format": "org.example.other", "formatted_body": "<b>x</b>"}});
        assert_eq!(select_html(Some(&raw), Some("<b>x</b>")), None);
        let no_format = json!({"content": {"msgtype": "m.text", "body": "b",
            "formatted_body": "<b>x</b>"}});
        assert_eq!(select_html(Some(&no_format), None), None);
    }

    #[test]
    fn source_with_a_missing_body() {
        let raw = json!({"content": {"msgtype": "m.text", "format": HTML}});
        assert_eq!(select_html(Some(&raw), None), None);
        let not_object = json!({"content": 5});
        assert_eq!(select_html(Some(&not_object), None), None);
        assert_eq!(select_html(Some(&json!({})), None), None);
        let wrong_type = json!({"content": {"format": HTML, "formatted_body": 5}});
        assert_eq!(select_html(Some(&wrong_type), None), None);
    }

    #[test]
    fn message_rich_text_picks_the_source_and_falls_back() {
        let raw = json!({"content": {"msgtype": "m.text", "body": "b", "format": HTML,
            "formatted_body": "<b>x</b><script>alert(1)</script>"}})
        .to_string();
        let rt = message_rich_text(Some(&raw), Some("<i>sdk</i>"), "b");
        assert_rt(&rt, &[p(vec![ts("x", style(|s| s.bold = true))])]);
        // Raw JSON without HTML: the plain body, not the SDK's.
        let plain = json!({"content": {"msgtype": "m.text", "body": "b"}}).to_string();
        assert_rt(
            &message_rich_text(Some(&plain), Some("<i>sdk</i>"), "b"),
            &[p(vec![t("b")])],
        );
        // Unparsable raw JSON counts as absent.
        let rt = message_rich_text(Some("{"), Some("<i>sdk</i>"), "b");
        assert_rt(&rt, &[p(vec![ts("sdk", style(|s| s.italic = true))])]);
        assert_rt(&message_rich_text(None, None, "b"), &[p(vec![t("b")])]);
    }

    #[test]
    fn source_without_raw_json_uses_the_sdk_body() {
        assert_eq!(
            select_html(None, Some("<b>x</b>")).as_deref(),
            Some("<b>x</b>")
        );
        assert_eq!(select_html(None, None), None);
    }
}
