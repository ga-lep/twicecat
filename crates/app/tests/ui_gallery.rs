//! The headless UI gallery (UI/UX rework plan, Task 5).
//!
//! Every screen of the app is built from a fixture `App` (see
//! `common/fixture.rs`: no core, no network, no audio device) and laid out
//! with tiny-skia, without a window, under every colour set x mode x density,
//! at 1280x800 and 720x800. The ignored test also draws each screen and
//! writes PNGs and an `index.html` for the human who picks the default
//! colour set.
//!
//! Limits: iced draws open `pick_list` menus and tooltips in its overlay
//! layer, which this harness does not draw, and the PNGs come from tiny-skia,
//! not the wgpu renderer of the real window. The gallery is an aid for
//! review, not proof of how the window looks.

#[path = "common/fixture.rs"]
mod fixture;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::Instant;

use iced::advanced::graphics::text::font_system;
use iced::advanced::layout::{Limits, Node};
use iced::advanced::renderer::{Headless, Style};
use iced::advanced::widget::Tree;
use iced::advanced::{Layout, Shell, clipboard, mouse};
use iced::{Element, Event, Pixels, Rectangle, Renderer, Size, window};
use twicecat_app::ui::fonts;
use twicecat_app::{App, Message};
use twicecat_core::api::{DensityChoice, PaletteChoice, RoomListChoice, ThemeChoice, UiSettings};

const WIDE: Size = Size::new(1280.0, 800.0);
const NARROW: Size = Size::new(720.0, 800.0);
const SIZES: [Size; 2] = [WIDE, NARROW];

const FAMILIES: [(PaletteChoice, &str); 4] = [
    (PaletteChoice::Twicecat, "twicecat"),
    (PaletteChoice::Frost, "frost"),
    (PaletteChoice::Ember, "ember"),
    (PaletteChoice::HighContrast, "high-contrast"),
];
const MODES: [(ThemeChoice, &str); 2] =
    [(ThemeChoice::Light, "light"), (ThemeChoice::Dark, "dark")];
const DENSITIES: [(DensityChoice, &str); 2] = [
    (DensityChoice::Comfortable, "comfortable"),
    (DensityChoice::Compact, "compact"),
];

/// One combination of colour set, mode, density and room list.
#[derive(Clone, Copy)]
struct Variant {
    family: (PaletteChoice, &'static str),
    mode: (ThemeChoice, &'static str),
    density: (DensityChoice, &'static str),
    list: RoomListChoice,
}

impl Variant {
    fn settings(self) -> UiSettings {
        UiSettings {
            theme: self.mode.0,
            palette: Some(self.family.0),
            density: self.density.0,
            room_list: self.list,
            ..UiSettings::default()
        }
    }

    /// `<family>-<mode>-<density>` plus `-rail` for the collapsed room list.
    fn tag(self) -> String {
        let rail = if self.list == RoomListChoice::Rail {
            "-rail"
        } else {
            ""
        };
        format!("{}-{}-{}{rail}", self.family.1, self.mode.1, self.density.1)
    }
}

/// Every variant of a screen: all 16, and 16 more with the rail for the
/// screens that show the room list.
fn variants(room_list: bool) -> Vec<Variant> {
    let lists: &[RoomListChoice] = if room_list {
        &[RoomListChoice::Expanded, RoomListChoice::Rail]
    } else {
        &[RoomListChoice::Expanded]
    };
    let mut all = Vec::new();
    for &list in lists {
        for mode in MODES {
            for density in DENSITIES {
                for family in FAMILIES {
                    all.push(Variant {
                        family,
                        mode,
                        density,
                        list,
                    });
                }
            }
        }
    }
    all
}

/// The headless renderer, with Inter loaded into the font system first
/// (the window loads it through `Application::font`).
fn renderer() -> Renderer {
    use std::sync::Once;
    static FONTS: Once = Once::new();
    FONTS.call_once(|| {
        let mut system = font_system().write().expect("the font system");
        for face in fonts::ALL {
            system.load_font(Cow::Borrowed(face));
        }
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    runtime
        .block_on(Renderer::new(fonts::BODY, Pixels(16.0), Some("tiny-skia")))
        .expect("the headless tiny-skia renderer is not available")
}

/// A screen laid out: the widget tree, its state and its layout.
struct Laid<'a> {
    element: Element<'a, Message>,
    tree: Tree,
    node: Node,
}

impl<'a> Laid<'a> {
    fn new(app: &'a App, renderer: &Renderer, size: Size) -> Self {
        let mut element = app.view();
        let mut tree = Tree::new(element.as_widget());
        let node =
            element
                .as_widget_mut()
                .layout(&mut tree, renderer, &Limits::new(Size::ZERO, size));
        Self {
            element,
            tree,
            node,
        }
    }

    /// Delivers one redraw to the widgets, as the window does before it
    /// draws. A button learns its status (active, disabled) in that event
    /// and draws as disabled until it has it, which made every label in the
    /// first gallery look faint.
    fn settle(&mut self, renderer: &Renderer, size: Size) {
        let mut messages: Vec<Message> = Vec::new();
        let mut shell = Shell::new(&mut messages);
        self.element.as_widget_mut().update(
            &mut self.tree,
            &Event::Window(window::Event::RedrawRequested(Instant::now())),
            Layout::new(&self.node),
            mouse::Cursor::Unavailable,
            renderer,
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::with_size(size),
        );
    }

    /// Draws into a fresh renderer and returns the RGBA pixels.
    fn pixels(&self, app: &App, mut renderer: Renderer, size: Size) -> Vec<u8> {
        let theme = app.current_theme();
        let palette = theme.palette();
        self.element.as_widget().draw(
            &self.tree,
            &mut renderer,
            &theme,
            &Style {
                text_color: palette.text,
            },
            Layout::new(&self.node),
            mouse::Cursor::Unavailable,
            &Rectangle::with_size(size),
        );
        let physical = Size::new(size.width as u32, size.height as u32);
        renderer.screenshot(physical, 1.0, palette.background)
    }
}

/// Where the gallery goes: `$TWICECAT_GALLERY_DIR` (a relative path is taken
/// from the workspace root), else `target/ui-gallery/` there.
fn gallery_dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    match std::env::var_os("TWICECAT_GALLERY_DIR") {
        Some(dir) if !dir.is_empty() => root.join(dir),
        _ => root.join("target/ui-gallery"),
    }
}

/// `index.html`: a plain `<img>` grid, one column per colour set, no scripts
/// and nothing remote. `shots` are `(screen, [(variant tag, file)])`.
fn index_html(shots: &[(&str, Vec<(String, String)>)]) -> String {
    let mut html = String::from(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <title>Twicecat UI gallery</title>\n<style>\n\
         body{font-family:sans-serif;margin:1rem}\n\
         table{border-collapse:collapse;margin-bottom:2rem}\n\
         th,td{border:1px solid #888;padding:4px;vertical-align:top}\n\
         img{width:320px;height:auto;display:block}\n\
         </style></head><body>\n<h1>Twicecat UI gallery</h1>\n\
         <p>tiny-skia, 1280x800, scale 1.0. Open menus and tooltips are not drawn. \
         Click a picture for the full size.</p>\n",
    );
    for (screen, files) in shots {
        html.push_str(&format!(
            "<h2 id=\"{screen}\">{screen}</h2>\n<table>\n<tr class=\"families\"><th></th>"
        ));
        for (_, name) in FAMILIES {
            html.push_str(&format!("<th>{name}</th>"));
        }
        html.push_str("</tr>\n");
        for mode in MODES {
            for density in DENSITIES {
                for rail in [false, true] {
                    let suffix = if rail { "-rail" } else { "" };
                    let row = format!("{}-{}{suffix}", mode.1, density.1);
                    let cells: Vec<String> = FAMILIES
                        .iter()
                        .filter_map(|(_, family)| {
                            let tag = format!("{family}-{row}");
                            files.iter().find(|(t, _)| *t == tag).map(|(_, file)| {
                                format!(
                                    "<td><a href=\"{file}\"><img src=\"{file}\" \
                                     alt=\"{screen} {tag}\" loading=\"lazy\"></a></td>"
                                )
                            })
                        })
                        .collect();
                    if cells.len() == FAMILIES.len() {
                        html.push_str(&format!("<tr><th>{row}</th>{}</tr>\n", cells.concat()));
                    }
                }
            }
        }
        html.push_str("</table>\n");
    }
    html.push_str("</body></html>\n");
    html
}

#[test]
fn every_screen_lays_out_in_every_variant_at_both_sizes() {
    let renderer = renderer();
    let scenarios = fixture::scenarios();
    assert!(scenarios.len() >= 30, "{} scenarios", scenarios.len());
    let mut laid_out = 0;
    for scenario in &scenarios {
        let mut app = (scenario.build)();
        for variant in variants(scenario.room_list) {
            app.set_ui_settings(variant.settings());
            for size in SIZES {
                app.window_width = size.width;
                let laid = Laid::new(&app, &renderer, size);
                let bounds = laid.node.bounds();
                assert!(
                    bounds.width > 0.0 && bounds.height > 0.0,
                    "{} {}",
                    scenario.name,
                    variant.tag()
                );
                laid_out += 1;
            }
        }
    }
    let per_screen: usize = scenarios
        .iter()
        .map(|s| variants(s.room_list).len() * SIZES.len())
        .sum();
    assert_eq!(laid_out, per_screen);
}

#[test]
fn the_fixture_reaches_every_screen_it_names() {
    for scenario in fixture::scenarios() {
        let app = (scenario.build)();
        assert!(
            (scenario.reached)(&app),
            "{} did not reach its state",
            scenario.name
        );
    }
    let names: Vec<&str> = fixture::scenarios().iter().map(|s| s.name).collect();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "scenario names are unique");
    for required in [
        "login",
        "waiting",
        "loading",
        "main-no-room",
        "main-room",
        "main-thread",
        "main-call-joining",
        "main-call-connected",
        "main-call-reconnecting",
        "main-call-unencrypted",
        "main-call-ptt-muted",
        "main-call-rows-open",
        "ring-long-id",
        "ring-dm",
        "ring-group",
        "audio-devices",
        "audio-talking",
        "audio-talking-capturing",
        "audio-microphone-gpu-found",
        "audio-microphone-gpu-none",
        "audio-microphone-gpu-looking",
        "audio-levels",
        "audio-mic-test",
        "audio-ringtone",
        "audio-other-voices",
        "audio-announce",
        "audio-diagnostics",
        "settings",
        "settings-appearance",
        "settings-notifications",
        "settings-voice-audio",
        "settings-keyboard",
        "settings-sessions",
        "settings-about",
        "sessions",
        "sessions-confirm-one",
        "sessions-confirm-all",
        "sessions-long-names",
        "sessions-reauth",
        "sessions-logout-warning",
        "room-settings",
        "members",
        "create-room",
        "join-room",
        "upload",
        "viewer",
        "switcher",
        "verification-requested",
        "verification-emojis",
        "verification-done",
        "recovery-key",
        "banner-device",
    ] {
        assert!(names.contains(&required), "missing scenario {required}");
    }
}

/// The path of child indices from the root to the main column
/// `[banner slot, panes, status bar]` (the panes are `[spaces, room list,
/// room pane, thread slot, call panel slot]`), and the shape of the room pane.
fn room_pane_place(laid: &Laid<'_>) -> (Vec<usize>, Vec<usize>) {
    let mut path = Vec::new();
    let mut node = &laid.node;
    loop {
        let kids = node.children();
        if kids.len() == 3 && kids[1].children().len() == 5 {
            let pane = &kids[1].children()[2];
            let shape = pane_shape(pane);
            path.extend([1, 2]);
            return (path, shape);
        }
        assert!(!kids.is_empty(), "no main column in the layout");
        path.push(0);
        node = &kids[0];
    }
}

/// Child counts down the room pane: container, then the column of
/// `[title, timeline, bars, editor]`.
fn pane_shape(pane: &Node) -> Vec<usize> {
    let column = &pane.children()[0];
    vec![pane.children().len(), column.children().len()]
}

/// A named change to the app.
type Step = (&'static str, fn(&mut App));

#[test]
fn the_room_pane_keeps_its_place_as_panels_bars_and_banners_come_and_go() {
    let renderer = renderer();
    let mut app = fixture::main_room();
    app.set_ui_settings(variants(false)[0].settings());
    let (place, shape) = {
        let laid = Laid::new(&app, &renderer, WIDE);
        room_pane_place(&laid)
    };
    assert_eq!(
        shape,
        vec![1, 4],
        "container around [title, timeline, bars, editor]"
    );
    let steps: [Step; 8] = [
        ("thread opens", fixture::thread_open),
        ("thread closes", fixture::thread_close),
        ("call starts", fixture::call_connected),
        ("call panel opens", fixture::call_panel_open),
        ("call panel closes", fixture::call_panel_close),
        ("call ends", fixture::call_left),
        ("banner appears", fixture::banner_on),
        ("banner goes", fixture::banner_off),
    ];
    for (what, step) in steps {
        step(&mut app);
        for size in SIZES {
            app.window_width = size.width;
            let laid = Laid::new(&app, &renderer, size);
            assert_eq!(
                room_pane_place(&laid),
                (place.clone(), shape.clone()),
                "{what}"
            );
        }
    }
    // All at once.
    fixture::thread_open(&mut app);
    fixture::call_connected(&mut app);
    fixture::call_panel_open(&mut app);
    fixture::banner_on(&mut app);
    for size in SIZES {
        app.window_width = size.width;
        let laid = Laid::new(&app, &renderer, size);
        assert_eq!(
            room_pane_place(&laid),
            (place.clone(), shape.clone()),
            "everything open at {size:?}"
        );
    }
}

/// Ctrl + mouse wheel over a closed `pick_list` with nothing selected picks
/// its last (wheel up) or first (wheel down) option. The account box has
/// nothing selected, so Log out must not be one of its options: Ctrl + wheel
/// is the zoom gesture.
#[test]
fn ctrl_wheel_over_the_status_bar_never_logs_out() {
    use iced::keyboard::{self, Modifiers};
    use iced::{Point, mouse::ScrollDelta};
    let renderer = renderer();
    let mut app = fixture::main_room();
    app.set_ui_settings(variants(false)[0].settings());
    let mut laid = Laid::new(&app, &renderer, WIDE);
    laid.settle(&renderer, WIDE);
    let mut messages: Vec<Message> = Vec::new();
    let mut deliver = |laid: &mut Laid<'_>, event: Event, at: Point| {
        let mut shell = Shell::new(&mut messages);
        laid.element.as_widget_mut().update(
            &mut laid.tree,
            &event,
            Layout::new(&laid.node),
            mouse::Cursor::Available(at),
            &renderer,
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::with_size(WIDE),
        );
    };
    let ctrl = Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::CTRL));
    deliver(&mut laid, ctrl, Point::new(1.0, 1.0));
    for y in [WIDE.height - 30.0, WIDE.height - 22.0, WIDE.height - 14.0] {
        for step in 0..160 {
            let at = Point::new(step as f32 * 8.0, y);
            for dy in [1.0, -1.0] {
                let wheel = Event::Mouse(mouse::Event::WheelScrolled {
                    delta: ScrollDelta::Lines { x: 0.0, y: dy },
                });
                deliver(&mut laid, wheel, at);
            }
        }
    }
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m, Message::LogoutRequested)),
        "Ctrl + wheel logged the user out: {messages:?}"
    );
}

/// The height a button takes when it may be `width` wide at most.
fn button_height(renderer: &Renderer, width: f32) -> f32 {
    use twicecat_app::ui::kit;
    let mut element: Element<'_, Message> =
        kit::danger_button("Leave call", Some(Message::LogoutRequested)).into();
    let mut tree = Tree::new(element.as_widget());
    let limits = Limits::new(Size::ZERO, Size::new(width, 400.0));
    element
        .as_widget_mut()
        .layout(&mut tree, renderer, &limits)
        .bounds()
        .height
}

#[test]
fn a_button_label_stays_on_one_line_when_the_header_is_squeezed() {
    let renderer = renderer();
    let roomy = button_height(&renderer, 400.0);
    let squeezed = button_height(&renderer, 50.0);
    assert!(
        (roomy - squeezed).abs() < 0.5,
        "the label wrapped: {roomy} px high when roomy, {squeezed} px when squeezed"
    );
}

#[test]
fn the_index_is_a_plain_image_grid() {
    let files: Vec<(String, String)> = variants(false)
        .iter()
        .map(|v| (v.tag(), format!("main-room-{}.png", v.tag())))
        .collect();
    let html = index_html(&[("main-room", files)]);
    assert!(!html.contains("<script"));
    assert!(!html.contains("http"));
    assert!(!html.contains("javascript"));
    for (_, family) in FAMILIES {
        assert!(html.contains(&format!("<th>{family}</th>")), "{family}");
    }
    assert_eq!(html.matches("<img ").count(), 16);
    assert_eq!(
        html.matches("<tr><th>").count(),
        4,
        "one row per mode x density"
    );
}

#[test]
fn variant_tags_are_file_name_safe() {
    let all = variants(true);
    assert_eq!(all.len(), 32);
    let mut tags: Vec<String> = all.iter().map(|v| v.tag()).collect();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(tags.len(), 32);
    assert!(tags.contains(&"frost-dark-compact-rail".to_owned()));
    assert!(tags.contains(&"high-contrast-light-comfortable".to_owned()));
}

/// Writes the gallery. Run on purpose:
/// `TWICECAT_GALLERY_DIR=target/ui-gallery cargo test -p twicecat-app
/// --test ui_gallery -- --ignored`.
#[test]
#[ignore = "writes PNGs for review"]
fn write_the_gallery() {
    let dir = gallery_dir();
    std::fs::create_dir_all(&dir).expect("create the gallery directory");
    let scenarios = fixture::scenarios();
    // `TWICECAT_GALLERY_ONLY=audio-` draws only the screens whose names start
    // with it (and not the narrow cases), for a quick look at one area.
    let only = std::env::var("TWICECAT_GALLERY_ONLY").unwrap_or_default();
    let mut shots: Vec<(&str, Vec<(String, String)>)> = Vec::new();
    for scenario in scenarios.iter().filter(|s| s.name.starts_with(&only)) {
        let mut app = (scenario.build)();
        let mut files = Vec::new();
        for variant in variants(scenario.room_list) {
            app.set_ui_settings(variant.settings());
            let renderer = renderer();
            let mut laid = Laid::new(&app, &renderer, WIDE);
            laid.settle(&renderer, WIDE);
            let pixels = laid.pixels(&app, renderer, WIDE);
            let file = format!("{}-{}.png", scenario.name, variant.tag());
            let (w, h) = (WIDE.width as u32, WIDE.height as u32);
            image::RgbaImage::from_raw(w, h, pixels)
                .expect("the screenshot has width x height x 4 bytes")
                .save(dir.join(&file))
                .expect("write the PNG");
            files.push((variant.tag(), file));
        }
        shots.push((scenario.name, files));
    }
    // Narrow and medium windows (Task 8): the list as a rail, no sidebar, a
    // panel in the room pane's place.
    let cases: [(&str, f32, Steps); 5] = [
        ("main-narrow-room", 720.0, &[]),
        ("main-narrow-thread", 720.0, &[fixture::thread_open]),
        (
            "main-narrow-call",
            720.0,
            &[fixture::call_connected, fixture::call_panel_open],
        ),
        ("main-medium-thread", 1000.0, &[fixture::thread_open]),
        (
            "main-medium-call",
            1000.0,
            &[fixture::call_connected, fixture::call_panel_open],
        ),
    ];
    for (name, width, steps) in cases.into_iter().filter(|_| only.is_empty()) {
        let size = Size::new(width, 800.0);
        let mut app = fixture::main_room();
        app.window_width = width;
        for step in steps {
            step(&mut app);
        }
        let mut files = Vec::new();
        for variant in variants(false) {
            app.set_ui_settings(variant.settings());
            let renderer = renderer();
            let mut laid = Laid::new(&app, &renderer, size);
            laid.settle(&renderer, size);
            let pixels = laid.pixels(&app, renderer, size);
            let file = format!("{name}-{}.png", variant.tag());
            image::RgbaImage::from_raw(width as u32, size.height as u32, pixels)
                .expect("the screenshot has width x height x 4 bytes")
                .save(dir.join(&file))
                .expect("write the PNG");
            files.push((variant.tag(), file));
        }
        shots.push((name, files));
    }
    std::fs::write(dir.join("index.html"), index_html(&shots)).expect("write index.html");
    println!("gallery written to {}", dir.display());
}

/// Click at `at` (press and release) and return the messages emitted.
fn click(laid: &mut Laid<'_>, renderer: &Renderer, size: Size, at: iced::Point) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::new();
    for event in [
        Event::Mouse(mouse::Event::CursorMoved { position: at }),
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
    ] {
        let mut shell = Shell::new(&mut messages);
        laid.element.as_widget_mut().update(
            &mut laid.tree,
            &event,
            Layout::new(&laid.node),
            mouse::Cursor::Available(at),
            renderer,
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::with_size(size),
        );
    }
    messages
}

/// The bounds of every node under `node`, translated to window coordinates.
fn collect_bounds(
    node: &Node,
    origin: iced::Point,
    out: &mut Vec<(Rectangle, usize)>,
    depth: usize,
) {
    let b = node.bounds();
    let at = iced::Point::new(origin.x + b.x, origin.y + b.y);
    out.push((Rectangle::new(at, b.size()), depth));
    for child in node.children() {
        collect_bounds(child, at, out, depth + 1);
    }
}

/// Finds the "Log out" button by clicking along the status bar: the real
/// button must emit `LogoutRequested`.
#[test]
fn clicking_the_log_out_button_requests_a_logout() {
    let renderer = renderer();
    let mut app = fixture::main_room();
    app.set_ui_settings(variants(false)[0].settings());
    let mut laid = Laid::new(&app, &renderer, WIDE);
    laid.settle(&renderer, WIDE);
    // The status bar is the last child of the main column; walk its nodes
    // (leaves first) and click the centre of each until one logs out.
    let mut all = Vec::new();
    collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
    let bar_top = WIDE.height - 60.0;
    let mut hits = 0;
    for (rect, _) in all.iter().filter(|(r, _)| r.y >= bar_top && r.width > 0.0) {
        let at = rect.center();
        let messages = click(&mut laid, &renderer, WIDE, at);
        if messages
            .iter()
            .any(|m| matches!(m, Message::LogoutRequested))
        {
            hits += 1;
            break;
        }
    }
    assert_eq!(hits, 1, "no click in the status bar logged out");
}

/// Tallest the header may be: avatar row, tags, the ID on three lines and one
/// line of buttons.
const MAX_HEADER_HEIGHT: f32 = 200.0;

const LONG_PEER: &str =
    "@a-very-long-user-name-that-goes-on-and-on-and-on-and-on-and-on:evil.example.org";

/// Nodes that spill out of their parent's bounds, as `(depth, bounds)`.
fn spills(node: &Node, origin: iced::Point, out: &mut Vec<Rectangle>) {
    let b = node.bounds();
    let at = iced::Point::new(origin.x + b.x, origin.y + b.y);
    let me = Rectangle::new(at, b.size());
    for child in node.children() {
        let cb = child.bounds();
        let cr = Rectangle::new(iced::Point::new(at.x + cb.x, at.y + cb.y), cb.size());
        if cr.x + cr.width > me.x + me.width + 0.5 || cr.y + cr.height > me.y + me.height + 0.5 {
            out.push(cr);
        }
        spills(child, at, out);
    }
}

/// The header is the first child of the room pane's column.
fn header_node<'n>(laid: &'n Laid<'_>) -> &'n Node {
    let mut node = &laid.node;
    loop {
        let kids = node.children();
        if kids.len() == 3 && kids[1].children().len() == 5 {
            return &kids[1].children()[2].children()[0].children()[0];
        }
        node = &kids[0];
    }
}

#[test]
fn a_narrow_header_never_clips_the_trust_tag_or_the_end_of_the_peer_id() {
    use twicecat_core::api::UserTrust;
    let renderer = renderer();
    for trust in [
        UserTrust::Verified,
        UserTrust::Unverified,
        UserTrust::IdentityChanged,
    ] {
        let mut app = fixture::dm_with_peer(LONG_PEER, trust);
        // Narrow windows: the room pane is what is left beside the spaces
        // bar and the room list (expanded, or the rail).
        for (list, width) in [
            (RoomListChoice::Expanded, 1280.0),
            (RoomListChoice::Expanded, 900.0),
            (RoomListChoice::Expanded, 720.0),
            (RoomListChoice::Expanded, 600.0),
            (RoomListChoice::Rail, 520.0),
            (RoomListChoice::Rail, 480.0),
            (RoomListChoice::Rail, 400.0),
            (RoomListChoice::Rail, 360.0),
        ] {
            let mut settings = variants(false)[0].settings();
            settings.room_list = list;
            app.set_ui_settings(settings);
            let size = Size::new(width, 800.0);
            app.window_width = width;
            let laid = Laid::new(&app, &renderer, size);
            let header = header_node(&laid);
            let mut out = Vec::new();
            spills(header, iced::Point::ORIGIN, &mut out);
            assert!(
                out.is_empty(),
                "{trust:?} at {width}: header content spills out: {out:?}"
            );
            // A header whose title block is squeezed to nothing wraps the
            // tag and the ID glyph by glyph and grows tall: nobody can read
            // the trust tag then, even though nothing "spills".
            let height = header.bounds().height;
            assert!(
                height <= MAX_HEADER_HEIGHT,
                "{trust:?} at {width}: the header is {height} px high"
            );
        }
    }
}

/// The room list pane's node: the first child of `[room list, divider]`.
fn list_pane_node<'n>(laid: &'n Laid<'_>) -> &'n Node {
    let mut node = &laid.node;
    loop {
        let kids = node.children();
        if kids.len() == 3 && kids[1].children().len() == 5 {
            return &kids[1].children()[1].children()[0];
        }
        assert!(!kids.is_empty(), "no main column in the layout");
        node = &kids[0];
    }
}

/// `[actions, tabs, search, list, call bar]`, inside the pane's container.
fn list_column(pane: &Node) -> &Node {
    &pane.children()[0]
}

#[test]
fn the_room_list_is_300_wide_or_a_72_px_rail_and_keeps_its_five_slots() {
    let renderer = renderer();
    let mut app = fixture::main_room();
    for density in DENSITIES {
        for (list, width) in [
            (RoomListChoice::Expanded, 300.0),
            (RoomListChoice::Rail, 72.0),
        ] {
            let mut settings = variants(false)[0].settings();
            settings.density = density.0;
            settings.room_list = list;
            app.set_ui_settings(settings);
            for in_call in [false, true] {
                if in_call {
                    fixture::call_connected(&mut app);
                } else {
                    fixture::call_left(&mut app);
                }
                for size in SIZES {
                    app.window_width = size.width;
                    let laid = Laid::new(&app, &renderer, size);
                    let pane = list_pane_node(&laid);
                    let what = format!("{list:?} {} call={in_call} {size:?}", density.1);
                    // A window under 800 px shows the rail whatever is saved.
                    let width = if size.width < 800.0 { 72.0 } else { width };
                    assert_eq!(pane.bounds().width, width, "{what}");
                    assert_eq!(list_column(pane).children().len(), 5, "{what}");
                    // Nothing in a slot spills out of the pane; the list's own
                    // content is taller than its view by design.
                    let column = list_column(pane);
                    let mut out = Vec::new();
                    for (i, slot) in column.children().iter().enumerate() {
                        let b = slot.bounds();
                        if b.x + b.width > column.bounds().width + 0.5 {
                            out.push(b);
                        }
                        if i != 3 {
                            spills(slot, iced::Point::ORIGIN, &mut out);
                        }
                    }
                    assert!(out.is_empty(), "{what}: spills {out:?}");
                }
            }
        }
    }
}

// ---- Room list rows: nothing overlaps (UI/UX rework, Task 7 wrap-up) ------

/// The room list's rows in window coordinates, and the width of the
/// scrollable's content (rows plus the gutter for the scrollbar).
fn list_rows<'n>(laid: &'n Laid<'_>) -> (Vec<(Rectangle, &'n Node)>, f32) {
    fn step(at: iced::Point, node: &Node) -> iced::Point {
        iced::Point::new(at.x + node.bounds().x, at.y + node.bounds().y)
    }
    let mut node = &laid.node;
    let mut at = step(iced::Point::ORIGIN, node);
    // The main column is `[header, panes row, status bar]`.
    while !(node.children().len() == 3 && node.children()[1].children().len() == 5) {
        assert!(!node.children().is_empty(), "no main column in the layout");
        node = &node.children()[0];
        at = step(at, node);
    }
    // panes row -> room list slot -> pane -> column
    // `[actions, tabs, search, list, call bar]`.
    let mut node = &node.children()[1];
    at = step(at, node);
    for index in [1, 0, 0] {
        node = &node.children()[index];
        at = step(at, node);
    }
    let mut content = &node.children()[3];
    at = step(at, content);
    while content.children().len() <= 1 {
        content = &content.children()[0];
        at = step(at, content);
    }
    let width = content.bounds().width;
    let rows = content
        .children()
        .iter()
        .filter(|row| row.bounds().height > 1.0 && row.bounds().width > 1.0)
        .map(|row| {
            let b = row.bounds();
            (
                Rectangle::new(iced::Point::new(at.x + b.x, at.y + b.y), b.size()),
                row,
            )
        })
        .collect();
    (rows, width)
}

/// Every node with no children and some area under `node`, in window
/// coordinates.
fn painted_leaves(node: &Node, at: iced::Point, out: &mut Vec<Rectangle>) {
    let b = node.bounds();
    let here = iced::Point::new(at.x + b.x, at.y + b.y);
    if node.children().is_empty() {
        if b.width > 0.0 && b.height > 0.0 {
            out.push(Rectangle::new(here, b.size()));
        }
        return;
    }
    for child in node.children() {
        painted_leaves(child, here, out);
    }
}

fn overlap(a: &Rectangle, b: &Rectangle) -> bool {
    a.intersection(b)
        .is_some_and(|i| i.width > 0.5 && i.height > 0.5)
}

fn crowded_variants() -> Vec<(DensityChoice, RoomListChoice)> {
    vec![
        (DensityChoice::Comfortable, RoomListChoice::Expanded),
        (DensityChoice::Compact, RoomListChoice::Expanded),
        (DensityChoice::Comfortable, RoomListChoice::Rail),
    ]
}

#[test]
fn no_two_things_in_a_room_list_row_overlap_and_none_reaches_the_scrollbar() {
    use twicecat_app::screens::room_list_pane::SCROLLBAR_GUTTER;
    let renderer = renderer();
    let mut app = fixture::crowded_room_list(&"W".repeat(120));
    for (density, list) in crowded_variants()
        .into_iter()
        .filter(|(_, list)| *list == RoomListChoice::Expanded)
    {
        let mut settings = variants(false)[0].settings();
        settings.density = density;
        settings.room_list = list;
        app.set_ui_settings(settings);
        let laid = Laid::new(&app, &renderer, WIDE);
        let (rows, content_width) = list_rows(&laid);
        assert!(rows.len() >= 4, "{density:?}: {} rows", rows.len());
        for (i, (rect, node)) in rows.iter().enumerate() {
            let mut leaves = Vec::new();
            painted_leaves(
                node,
                iced::Point::new(rect.x - node.bounds().x, rect.y - node.bounds().y),
                &mut leaves,
            );
            let what = format!("{density:?} row {i} at 300 px");
            assert!(
                rect.x + rect.width
                    <= rect.x - node.bounds().x + content_width - SCROLLBAR_GUTTER + 0.5,
                "{what}: the row reaches into the scrollbar's gutter: {rect:?}, content {content_width}"
            );
            for leaf in &leaves {
                assert!(
                    leaf.x + leaf.width <= rect.x + rect.width + 0.5
                        && leaf.y + leaf.height <= rect.y + rect.height + 0.5,
                    "{what}: {leaf:?} sticks out of the row {rect:?}"
                );
            }
            for (a, first) in leaves.iter().enumerate() {
                for second in &leaves[a + 1..] {
                    assert!(
                        !overlap(first, second),
                        "{what}: {first:?} overlaps {second:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn rail_marks_and_dots_never_overlap_each_other() {
    let renderer = renderer();
    let mut app = fixture::crowded_room_list("a message");
    for density in DENSITIES {
        let mut settings = variants(false)[0].settings();
        settings.density = density.0;
        settings.room_list = RoomListChoice::Rail;
        app.set_ui_settings(settings);
        let laid = Laid::new(&app, &renderer, WIDE);
        let (rows, _) = list_rows(&laid);
        assert!(rows.len() >= 4, "{} rows", rows.len());
        let mut most = 0;
        for (i, (rect, node)) in rows.iter().enumerate() {
            let origin = iced::Point::new(rect.x - node.bounds().x, rect.y - node.bounds().y);
            // The picture is a stack of the avatar and its layers: four
            // children of one size.
            let mut stack = None;
            find_stack(node, origin, &mut stack);
            let (avatar, layers) = stack.unwrap_or_else(|| panic!("row {i}: no picture"));
            let mut chips = Vec::new();
            for layer in layers {
                small_nodes(layer.0, layer.1, &mut chips);
            }
            most = most.max(chips.len());
            for chip in &chips {
                assert!(
                    chip.x >= avatar.x - 0.5
                        && chip.y >= avatar.y - 0.5
                        && chip.x + chip.width <= avatar.x + avatar.width + 0.5
                        && chip.y + chip.height <= avatar.y + avatar.height + 0.5,
                    "{} row {i}: {chip:?} is not on the avatar {avatar:?}",
                    density.1
                );
            }
            for (a, first) in chips.iter().enumerate() {
                for second in &chips[a + 1..] {
                    assert!(
                        !overlap(first, second),
                        "{} row {i}: {first:?} overlaps {second:?}",
                        density.1
                    );
                }
            }
        }
        assert!(most >= 4, "the crowded room shows {most} marks only");
    }
}

/// The picture stack of a rail row: the avatar's rectangle and the layers
/// above it, with their origins.
type Layers<'n> = (Rectangle, Vec<(&'n Node, iced::Point)>);

fn find_stack<'n>(node: &'n Node, origin: iced::Point, out: &mut Option<Layers<'n>>) {
    let b = node.bounds();
    let here = iced::Point::new(origin.x + b.x, origin.y + b.y);
    let kids = node.children();
    if kids.len() == 4
        && kids
            .iter()
            .all(|k| k.bounds().size() == kids[0].bounds().size())
    {
        let k0 = kids[0].bounds();
        let avatar = Rectangle::new(iced::Point::new(here.x + k0.x, here.y + k0.y), k0.size());
        *out = Some((avatar, kids[1..].iter().map(|k| (k, here)).collect()));
        return;
    }
    for child in kids {
        find_stack(child, here, out);
    }
}

/// The outermost nodes at most 16 px each way (the largest a rail chip has been), with some area.
fn small_nodes(node: &Node, origin: iced::Point, out: &mut Vec<Rectangle>) {
    let b = node.bounds();
    let here = iced::Point::new(origin.x + b.x, origin.y + b.y);
    if b.width > 0.0 && b.height > 0.0 && b.width <= 16.5 && b.height <= 16.5 {
        out.push(Rectangle::new(here, b.size()));
        return;
    }
    for child in node.children() {
        small_nodes(child, here, out);
    }
}

#[test]
fn a_long_name_or_preview_is_cut_at_its_slot_and_never_drawn_under_the_tags_or_the_scrollbar() {
    let pixels = |text: &str, density: DensityChoice| {
        let renderer = renderer();
        let mut app = fixture::crowded_room_list(text);
        let mut settings = variants(false)[0].settings();
        settings.density = density;
        app.set_ui_settings(settings);
        let mut laid = Laid::new(&app, &renderer, WIDE);
        laid.settle(&renderer, WIDE);
        let slots = {
            let (rows, _) = list_rows(&laid);
            rows.iter()
                .map(|(rect, node)| {
                    // [accent bar, button]; the button's child is
                    // [avatar, body]; the body is [title, preview + tags]
                    // (Comfortable) or the title row (Compact).
                    let mut at = iced::Point::new(rect.x, rect.y);
                    let mut step = |n: &Node| {
                        at = iced::Point::new(at.x + n.bounds().x, at.y + n.bounds().y);
                    };
                    let button = &node.children()[1];
                    step(button);
                    let content = &button.children()[0];
                    step(content);
                    let body = &content.children()[1];
                    step(body);
                    let name_slot = if density == DensityChoice::Comfortable {
                        &body.children()[0].children()[0]
                    } else {
                        &body.children()[0]
                    };
                    let preview_slot = (density == DensityChoice::Comfortable)
                        .then(|| &body.children()[1].children()[0]);
                    let place = |slot: &Node, extra: iced::Point| {
                        Rectangle::new(
                            iced::Point::new(extra.x + slot.bounds().x, extra.y + slot.bounds().y),
                            slot.bounds().size(),
                        )
                    };
                    let name = if density == DensityChoice::Comfortable {
                        let title = &body.children()[0];
                        place(
                            name_slot,
                            iced::Point::new(at.x + title.bounds().x, at.y + title.bounds().y),
                        )
                    } else {
                        place(name_slot, at)
                    };
                    let preview = preview_slot.map(|p| {
                        let lower = &body.children()[1];
                        place(
                            p,
                            iced::Point::new(at.x + lower.bounds().x, at.y + lower.bounds().y),
                        )
                    });
                    (*rect, name, preview)
                })
                .collect::<Vec<_>>()
        };
        let image = laid.pixels(&app, renderer, WIDE);
        (image, slots)
    };
    for density in [DensityChoice::Comfortable, DensityChoice::Compact] {
        let (wide, slots) = pixels(&"W".repeat(120), density);
        let (thin, _) = pixels(&"i".repeat(120), density);
        assert_eq!(wide.len(), thin.len());
        let mut checked = 0;
        for (row, name, preview) in &slots {
            // Right of a slot, to the end of the row (the scrollbar's side
            // included), only things that do not depend on the text are drawn.
            let mut regions = vec![name];
            regions.extend(preview.as_ref());
            for slot in regions {
                let left = (slot.x + slot.width).ceil() as usize;
                let right = (row.x + row.width).ceil() as usize + 14;
                let (top, bottom) = (slot.y as usize, (slot.y + slot.height) as usize);
                let mut differing = 0;
                for y in top..bottom {
                    for x in left..right.min(WIDE.width as usize) {
                        let i = (y * WIDE.width as usize + x) * 4;
                        if wide[i..i + 4] != thin[i..i + 4] {
                            differing += 1;
                        }
                    }
                }
                assert_eq!(
                    differing, 0,
                    "{density:?}: text drawn right of its slot {slot:?} ({differing} pixels)"
                );
                checked += 1;
            }
        }
        assert!(checked >= 4, "{density:?}: {checked} slots checked");
    }
}

#[test]
fn ten_thousand_rooms_build_only_the_rows_in_view_for_every_density_and_the_rail() {
    use twicecat_core::api::{RoomId, RoomSummary};
    let renderer = renderer();
    let mut app = fixture::main_room();
    app.rooms = (0..10_000)
        .map(|i| {
            let mut room = RoomSummary::new(
                RoomId::from(format!("!r{i}:example.org")),
                format!("Room {i}"),
            );
            room.unread_notifications = (i % 4) as u64;
            room
        })
        .collect();
    app.list_viewport_h = 700.0;
    let mut built = Vec::new();
    for (density, list) in [
        (DensityChoice::Comfortable, RoomListChoice::Expanded),
        (DensityChoice::Compact, RoomListChoice::Expanded),
        (DensityChoice::Comfortable, RoomListChoice::Rail),
    ] {
        let mut settings = variants(false)[0].settings();
        settings.density = density;
        settings.room_list = list;
        app.set_ui_settings(settings);
        app.list_offset_y = 4_000.0 * app.room_row_height();
        let laid = Laid::new(&app, &renderer, WIDE);
        let pane = list_pane_node(&laid);
        // The scrollable's content: [top spacer, rows..., bottom spacer].
        let rows = scrollable_rows(list_column(pane), app.room_row_height());
        assert!(rows < 100, "{density:?} {list:?}: {rows} rows built");
        built.push(rows);
    }
    assert!(built[1] > built[0], "Compact rows are shorter, so more fit");
}

/// The number of rows (children one row high) under the list slot's
/// scrollable; the spacers around them are taller or absent.
fn scrollable_rows(column: &Node, row_height: f32) -> usize {
    let mut node = &column.children()[3];
    // scrollable -> content column
    while node.children().len() <= 1 {
        node = &node.children()[0];
    }
    node.children()
        .iter()
        .filter(|row| (row.bounds().height - row_height).abs() < 0.5)
        .count()
}

/// The panes row `[spaces, room list, room pane, thread slot, call slot]`.
fn panes_row<'n>(laid: &'n Laid<'_>) -> &'n Node {
    let mut node = &laid.node;
    loop {
        let kids = node.children();
        if kids.len() == 3 && kids[1].children().len() == 5 {
            return &kids[1];
        }
        assert!(!kids.is_empty(), "no main column in the layout");
        node = &kids[0];
    }
}

/// Changes to the app, in order.
type Steps = &'static [fn(&mut App)];

/// How the side panels are opened in a test.
type Panels = (&'static str, Steps);

const PANEL_STATES: [Panels; 5] = [
    ("none", &[]),
    ("thread", &[fixture::thread_open]),
    ("call", &[fixture::call_connected, fixture::call_panel_open]),
    (
        "thread then call",
        &[
            fixture::thread_open,
            fixture::call_connected,
            fixture::call_panel_open,
        ],
    ),
    (
        "call then thread",
        &[
            fixture::call_connected,
            fixture::call_panel_open,
            fixture::thread_open,
        ],
    ),
];

#[test]
fn the_slots_follow_the_layout_at_every_width_and_the_room_pane_keeps_its_place() {
    let renderer = renderer();
    let mut place = None;
    for width in [1280.0, 1100.0, 1000.0, 800.0, 720.0, 500.0] {
        let size = Size::new(width, 800.0);
        for (what, steps) in PANEL_STATES {
            for list in [RoomListChoice::Expanded, RoomListChoice::Rail] {
                let mut app = fixture::main_room();
                let mut settings = variants(false)[0].settings();
                settings.room_list = list;
                app.set_ui_settings(settings);
                app.window_width = width;
                for step in steps {
                    step(&mut app);
                }
                let shown = app.pane_layout();
                let what = format!("{what} {list:?} at {width}");
                let laid = Laid::new(&app, &renderer, size);
                let here = room_pane_place(&laid);
                assert_eq!(
                    *place.get_or_insert_with(|| here.clone()),
                    here,
                    "{what}: the room pane moved in the tree"
                );
                let row = panes_row(&laid);
                let w: Vec<f32> = row.children().iter().map(|n| n.bounds().width).collect();
                assert_eq!(w.len(), 5, "{what}");
                assert_eq!(w[0] > 0.0, shown.sidebar, "{what}: sidebar {w:?}");
                let list_w = if shown.list == RoomListChoice::Rail {
                    73.0
                } else {
                    301.0
                };
                assert_eq!(w[1], list_w, "{what}: list {w:?}");
                assert_eq!(w[3] > 0.0, shown.thread, "{what}: thread {w:?}");
                assert_eq!(w[4] > 0.0, shown.call, "{what}: call {w:?}");
                assert_eq!(w[2] == 0.0, shown.panel_fills, "{what}: room pane {w:?}");
                let sum: f32 = w.iter().sum();
                assert!(
                    (sum - width).abs() < 0.5,
                    "{what}: the slots add up to {sum}, not {width}: {w:?}"
                );
                if shown.panel_fills {
                    let panel = if shown.thread { w[3] } else { w[4] };
                    assert_eq!(panel, width - w[0] - w[1], "{what}: fills {w:?}");
                }
            }
        }
    }
}

/// Ctrl + wheel is the zoom gesture, and a `pick_list` with a value takes it
/// as "pick the next option": over the header's notification menu that would
/// silently mute a room. Sweep the top of the window, wide and narrow.
#[test]
fn ctrl_wheel_over_the_room_header_never_changes_the_notification_mode() {
    use iced::keyboard::{self, Modifiers};
    use iced::{Point, mouse::ScrollDelta};
    let renderer = renderer();
    for width in [1280.0, 900.0, 600.0, 400.0] {
        let size = Size::new(width, 800.0);
        let mut app = fixture::main_room();
        app.set_ui_settings(variants(false)[0].settings());
        app.window_width = width;
        let mut laid = Laid::new(&app, &renderer, size);
        laid.settle(&renderer, size);
        let mut messages: Vec<Message> = Vec::new();
        let mut deliver = |laid: &mut Laid<'_>, event: Event, at: Point| {
            let mut shell = Shell::new(&mut messages);
            laid.element.as_widget_mut().update(
                &mut laid.tree,
                &event,
                Layout::new(&laid.node),
                mouse::Cursor::Available(at),
                &renderer,
                &mut clipboard::Null,
                &mut shell,
                &Rectangle::with_size(size),
            );
        };
        let ctrl = Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::CTRL));
        deliver(&mut laid, ctrl, Point::new(1.0, 1.0));
        for y in (0..200).step_by(4) {
            for x in (0..width as i32).step_by(6) {
                let at = Point::new(x as f32, y as f32);
                for dy in [1.0, -1.0] {
                    let wheel = Event::Mouse(mouse::Event::WheelScrolled {
                        delta: ScrollDelta::Lines { x: 0.0, y: dy },
                    });
                    deliver(&mut laid, wheel, at);
                }
            }
        }
        let changes = messages
            .iter()
            .filter(|m| matches!(m, Message::RoomModeChosen(_)))
            .count();
        assert_eq!(
            changes, 0,
            "Ctrl + wheel changed a room's notification mode at {width}"
        );
    }
}

#[test]
fn a_panel_that_hides_the_room_header_carries_the_trust_tag() {
    use twicecat_core::api::UserTrust;
    let renderer = renderer();
    // `[title, (trust tag), close]` in the header of the thread panel.
    let head_len = |width: f32| {
        let mut app = fixture::dm_with_peer(LONG_PEER, UserTrust::IdentityChanged);
        app.window_width = width;
        fixture::thread_open(&mut app);
        let laid = Laid::new(&app, &renderer, Size::new(width, 800.0));
        let slot = &panes_row(&laid).children()[3];
        assert!(slot.bounds().width > 0.0, "the thread panel is shown");
        // Row `[divider, container]`, container -> column `[header, ..]`.
        slot.children()[1].children()[0].children()[0]
            .children()
            .len()
    };
    assert_eq!(
        head_len(1280.0),
        2,
        "the room header shows it beside a panel"
    );
    assert_eq!(
        head_len(700.0),
        3,
        "the panel shows it when the header is hidden"
    );
}

#[test]
fn a_header_with_a_call_never_spills_or_grows_tall_when_narrow() {
    let renderer = renderer();
    for width in [1280.0, 900.0, 600.0, 480.0, 400.0, 360.0] {
        let size = Size::new(width, 800.0);
        let mut app = fixture::main_room();
        app.set_ui_settings(variants(false)[0].settings());
        app.window_width = width;
        fixture::call_connected(&mut app);
        let laid = Laid::new(&app, &renderer, size);
        let header = header_node(&laid);
        let mut out = Vec::new();
        spills(header, iced::Point::ORIGIN, &mut out);
        assert!(out.is_empty(), "at {width}: header content spills: {out:?}");
        let height = header.bounds().height;
        assert!(
            height <= MAX_HEADER_HEIGHT,
            "at {width}: the header is {height} px high"
        );
    }
}

/// A closed `pick_list` with a value takes Ctrl + wheel (the zoom gesture) as
/// "pick the next option". On the voice settings that swaps the microphone
/// or turns push-to-talk into an open microphone, so no menu may take it:
/// sweep every screen with a menu.
#[test]
fn ctrl_wheel_over_the_menus_of_the_settings_and_call_screens_changes_nothing() {
    use iced::keyboard::{self, Modifiers};
    use iced::{Point, mouse::ScrollDelta};
    let renderer = renderer();
    let mut apps: Vec<(String, App)> = [
        "audio-devices",
        "audio-talking",
        "audio-talking-capturing",
        "audio-talking-voice-activity",
        "audio-microphone-gpu-found",
        "audio-levels",
        "audio-mic-test",
        "audio-other-voices",
        "settings",
        "settings-appearance",
        "settings-notifications",
        "sessions",
        "sessions-confirm-all",
    ]
    .into_iter()
    .map(|name| {
        let scenario = fixture::scenarios()
            .into_iter()
            .find(|s| s.name == name)
            .expect("a scenario of that name");
        (name.to_owned(), (scenario.build)())
    })
    .collect();
    let mut with_panel = fixture::main_room();
    fixture::call_connected(&mut with_panel);
    fixture::call_panel_open(&mut with_panel);
    apps.push(("call panel".to_owned(), with_panel));
    // Open rows show the volume slider and the cleaning menu.
    apps.push((
        "call panel, rows open".to_owned(),
        scenario_app("main-call-rows-open"),
    ));
    apps.push((
        "call dock, push-to-talk".to_owned(),
        scenario_app("main-call-ptt-idle"),
    ));
    for (name, app) in &apps {
        let mut laid = Laid::new(app, &renderer, WIDE);
        laid.settle(&renderer, WIDE);
        let mut messages: Vec<Message> = Vec::new();
        let mut deliver = |laid: &mut Laid<'_>, event: Event, at: Point| {
            let mut shell = Shell::new(&mut messages);
            laid.element.as_widget_mut().update(
                &mut laid.tree,
                &event,
                Layout::new(&laid.node),
                mouse::Cursor::Available(at),
                &renderer,
                &mut clipboard::Null,
                &mut shell,
                &Rectangle::with_size(WIDE),
            );
        };
        let ctrl = Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::CTRL));
        deliver(&mut laid, ctrl, Point::new(1.0, 1.0));
        for y in (0..800).step_by(5) {
            for x in (0..1280).step_by(8) {
                let at = Point::new(x as f32, y as f32);
                for dy in [1.0, -1.0] {
                    // A scrollable keeps wheel events from what is inside it
                    // for a while after it scrolled; leaving it ends that.
                    let left = Event::Mouse(mouse::Event::CursorLeft);
                    deliver(&mut laid, left, at);
                    let wheel = Event::Mouse(mouse::Event::WheelScrolled {
                        delta: ScrollDelta::Lines { x: 0.0, y: dy },
                    });
                    deliver(&mut laid, wheel, at);
                }
            }
        }
        let changed: Vec<String> = messages
            .iter()
            .map(|m| format!("{m:?}"))
            .filter(|m| {
                ["Audio(", "Settings(", "Call(", "RoomModeChosen"]
                    .iter()
                    .any(|p| m.starts_with(p))
            })
            .collect();
        assert!(
            changed.is_empty(),
            "Ctrl + wheel changed a setting on {name}: {changed:?}"
        );
    }
}

// ---- Task 11: the call dock, the participants panel and the ring card -----

fn scenario_app(name: &str) -> App {
    let scenario = fixture::scenarios()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no scenario {name}"));
    (scenario.build)()
}

/// The slot after the list: the dock (a column of its blocks).
fn dock_slot<'n>(laid: &'n Laid<'_>) -> &'n Node {
    &list_column(list_pane_node(laid)).children()[4]
}

/// The lines of the dock's card: the dock column's first block is the card's
/// container, around a column of lines.
fn dock_lines(dock: &Node) -> &[Node] {
    dock.children()[0].children()[0].children()
}

fn with_settings(app: &mut App, density: (DensityChoice, &str), list: RoomListChoice) {
    let mut settings = variants(false)[0].settings();
    settings.density = density.0;
    settings.room_list = list;
    app.set_ui_settings(settings);
    app.window_width = WIDE.width;
}

const CALL_SCENARIOS: [&str; 8] = [
    "main-call-joining",
    "main-call-connected",
    "main-call-reconnecting",
    "main-call-unencrypted",
    "main-call-ptt-idle",
    "main-call-ptt-talking",
    "main-call-ptt-muted",
    "main-call-notice",
];

#[test]
fn the_expanded_dock_never_spills_and_keeps_leave_apart_at_300_px() {
    let renderer = renderer();
    for name in CALL_SCENARIOS {
        for density in DENSITIES {
            let mut app = scenario_app(name);
            with_settings(&mut app, density, RoomListChoice::Expanded);
            let laid = Laid::new(&app, &renderer, WIDE);
            let column = list_column(list_pane_node(&laid));
            let dock = dock_slot(&laid);
            let what = format!("{name} {}", density.1);
            assert_eq!(column.bounds().width, 300.0 - 2.0 * tokens_s(), "{what}");
            assert!(
                (dock.bounds().width - column.bounds().width).abs() < 0.5,
                "{what}: the dock is {} wide",
                dock.bounds().width
            );
            let mut out = Vec::new();
            spills(dock, iced::Point::ORIGIN, &mut out);
            assert!(out.is_empty(), "{what}: spills {out:?}");
            if name == "main-call-notice" {
                // A notice alone: a banner, no call card.
                assert!(dock.bounds().height > 0.0, "{what}");
                continue;
            }
            let lines = dock_lines(dock);
            let controls = lines.last().expect("the controls row");
            // [mic, deafen, audio, spacer, leave]
            let kids = controls.children();
            assert_eq!(kids.len(), 5, "{what}");
            let (audio, leave) = (kids[2].bounds(), kids[4].bounds());
            assert!(
                leave.x - (audio.x + audio.width) >= 15.5,
                "{what}: Leave is {} px from Audio",
                leave.x - (audio.x + audio.width)
            );
            assert!(
                leave.x + leave.width <= controls.bounds().width + 0.5,
                "{what}: Leave runs out of the row"
            );
            for (i, kid) in kids.iter().enumerate() {
                assert!(
                    kid.bounds().x + kid.bounds().width <= controls.bounds().width + 0.5,
                    "{what}: control {i} is cut off"
                );
            }
            // The Audio button is an icon button, no wider than a normal one.
            assert!(audio.width <= 48.0, "{what}: Audio is {} wide", audio.width);
        }
    }
}

/// The pane's inner padding, for the arithmetic of the width check.
fn tokens_s() -> f32 {
    twicecat_app::ui::tokens::S
}

#[test]
fn a_long_room_name_wraps_in_the_dock_instead_of_being_cut() {
    let renderer = renderer();
    let mut short = scenario_app("main-call-connected");
    let mut long = scenario_app("main-call-connected");
    let name = "W".repeat(150);
    for room in &mut long.rooms {
        if room.id.as_str().starts_with("!lounge") {
            room.name = name.clone();
        }
    }
    let heights: Vec<f32> = [&mut short, &mut long]
        .into_iter()
        .map(|app| {
            with_settings(app, DENSITIES[0], RoomListChoice::Expanded);
            let laid = Laid::new(app, &renderer, WIDE);
            let dock = dock_slot(&laid);
            let mut out = Vec::new();
            spills(dock, iced::Point::ORIGIN, &mut out);
            assert!(out.is_empty(), "spills {out:?}");
            dock.bounds().height
        })
        .collect();
    assert!(
        heights[1] >= heights[0] + 20.0,
        "a long unbroken name takes more lines: {heights:?}"
    );
}

#[test]
fn the_dock_slot_keeps_its_place_with_and_without_a_call() {
    let renderer = renderer();
    let mut app = scenario_app("main-call-connected");
    for list in [RoomListChoice::Expanded, RoomListChoice::Rail] {
        with_settings(&mut app, DENSITIES[0], list);
        for connected in [true, false] {
            if connected {
                fixture::call_connected(&mut app);
            } else {
                fixture::call_left(&mut app);
            }
            let laid = Laid::new(&app, &renderer, WIDE);
            let column = list_column(list_pane_node(&laid));
            assert_eq!(column.children().len(), 5, "{list:?} call={connected}");
            assert_eq!(
                dock_slot(&laid).bounds().height > 0.0,
                connected,
                "{list:?} call={connected}"
            );
        }
    }
}

#[test]
fn the_rail_dock_fits_72_px_and_keeps_leave_apart_from_deafen() {
    let renderer = renderer();
    for name in CALL_SCENARIOS {
        for density in DENSITIES {
            let mut app = scenario_app(name);
            with_settings(&mut app, density, RoomListChoice::Rail);
            let laid = Laid::new(&app, &renderer, WIDE);
            let pane = list_pane_node(&laid);
            assert_eq!(pane.bounds().width, 72.0);
            let dock = dock_slot(&laid);
            let what = format!("{name} {}", density.1);
            let mut out = Vec::new();
            spills(dock, iced::Point::ORIGIN, &mut out);
            assert!(out.is_empty(), "{what}: spills {out:?}");
            assert!(
                dock.bounds().width <= list_column(pane).bounds().width + 0.5,
                "{what}"
            );
            if name == "main-call-notice" {
                continue;
            }
            // The card's column: [phase, lock?, declined?, mic, deafen, gap, leave].
            let items = dock.children()[0].children();
            let n = items.len();
            let leave = items[n - 1].bounds();
            // The control above Leave (skipping the zero-width gap).
            let above = items[..n - 1]
                .iter()
                .rev()
                .map(Node::bounds)
                .find(|b| b.width > 0.0)
                .expect("a control above Leave");
            assert!(
                leave.y - (above.y + above.height) >= 12.0,
                "{what}: Leave is {} px below the control above",
                leave.y - (above.y + above.height)
            );
        }
    }
}

/// The panes row of the main view: `[spaces, list, room, thread, call]`.
fn panes_node<'n>(laid: &'n Laid<'_>) -> &'n Node {
    let mut node = &laid.node;
    loop {
        let kids = node.children();
        if kids.len() == 3 && kids[1].children().len() == 5 {
            return &kids[1];
        }
        assert!(!kids.is_empty(), "no main column in the layout");
        node = &kids[0];
    }
}

/// Every node under `node` with `count` children, depth first.
fn nodes_with<'n>(node: &'n Node, count: usize, out: &mut Vec<&'n Node>) {
    if node.children().len() == count {
        out.push(node);
    }
    for child in node.children() {
        nodes_with(child, count, out);
    }
}

#[test]
fn the_participants_panel_is_320_wide_and_its_open_rows_have_room_for_the_slider() {
    let renderer = renderer();
    for density in DENSITIES {
        for name in ["main-call-connected", "main-call-rows-open"] {
            let mut app = scenario_app(name);
            with_settings(&mut app, density, RoomListChoice::Expanded);
            let laid = Laid::new(&app, &renderer, WIDE);
            let slot = &panes_node(&laid).children()[4];
            let what = format!("{name} {}", density.1);
            assert_eq!(slot.bounds().width, 320.0, "{what}");
            let mut out = Vec::new();
            spills(slot, iced::Point::ORIGIN, &mut out);
            assert!(out.is_empty(), "{what}: spills {out:?}");
            // Every row's [picture, details, More]: the words get real room
            // and are not covered by the button.
            let mut tops = Vec::new();
            nodes_with(slot, 3, &mut tops);
            let tops: Vec<&Node> = tops
                .into_iter()
                .filter(|n| {
                    let first = n.children()[0].bounds();
                    first.width > 38.0 && first.width < 42.0 && first.height < 44.0
                })
                .collect();
            assert!(!tops.is_empty(), "{what}: no participant row found");
            for top in &tops {
                let (picture, details, more) = (
                    top.children()[0].bounds(),
                    top.children()[1].bounds(),
                    top.children()[2].bounds(),
                );
                assert!(
                    details.x >= picture.x + picture.width,
                    "{what}: details cover the picture"
                );
                assert!(
                    details.x + details.width <= more.x + 0.5,
                    "{what}: the More button covers the words"
                );
                assert!(details.width >= 120.0, "{what}: details {details:?}");
            }
            // The volume row: [-, slider, +, percent].
            let mut sliders = Vec::new();
            nodes_with(slot, 4, &mut sliders);
            let sliders: Vec<&&Node> = sliders
                .iter()
                .filter(|n| {
                    let slider = n.children()[1].bounds();
                    slider.width > 40.0 && n.children()[3].bounds().width == 44.0
                })
                .collect();
            let open = if name == "main-call-rows-open" { 2 } else { 0 };
            assert_eq!(sliders.len(), open, "{what}: open volume rows");
            for row in sliders {
                assert!(
                    row.children()[1].bounds().width >= 100.0,
                    "{what}: the slider is {} wide",
                    row.children()[1].bounds().width
                );
            }
        }
    }
}

#[test]
fn the_ring_card_keeps_decline_and_accept_apart_below_the_words() {
    let renderer = renderer();
    let mut heights = Vec::new();
    for name in ["ring-dm", "ring-group", "ring-long-id"] {
        for size in SIZES {
            let mut app = scenario_app(name);
            with_settings(&mut app, DENSITIES[0], RoomListChoice::Expanded);
            app.window_width = size.width;
            let laid = Laid::new(&app, &renderer, size);
            let what = format!("{name} {size:?}");
            // The root is the stack [base, card layer]; the layer is an
            // opaque container around the card.
            let layer = laid.node.children().last().expect("the card layer");
            let mut out = Vec::new();
            spills(layer, iced::Point::ORIGIN, &mut out);
            assert!(out.is_empty(), "{what}: spills {out:?}");
            let mut pairs = Vec::new();
            nodes_with(layer, 2, &mut pairs);
            let buttons: Vec<&&Node> = pairs
                .iter()
                .filter(|n| {
                    let (a, b) = (n.children()[0].bounds(), n.children()[1].bounds());
                    (a.y - b.y).abs() < 0.5
                        && (a.width - b.width).abs() < 1.0
                        && a.width > 100.0
                        && a.height < 80.0
                        && a.height > 20.0
                })
                .collect();
            let row = buttons.last().expect("the Decline and Accept row");
            let (decline, accept) = (row.children()[0].bounds(), row.children()[1].bounds());
            assert!(
                accept.x - (decline.x + decline.width) >= 15.5,
                "{what}: gap {}",
                accept.x - (decline.x + decline.width)
            );
            assert!(decline.x < accept.x, "{what}: Decline comes first");
            if size == WIDE {
                heights.push((name, layer.children()[0].bounds().height));
            }
        }
    }
    // A user ID with no break point wraps by glyph: the card grows instead of
    // the end of the ID being cut off.
    let height = |n: &str| heights.iter().find(|(m, _)| *m == n).expect("height").1;
    // Glyph breaks wrap the one long word over more lines than Word wrapping,
    // which leaves it running past the edge of the card.
    assert!(
        height("ring-long-id") >= height("ring-group") + 45.0,
        "{heights:?}"
    );
}

#[test]
fn a_ring_answered_keeps_both_buttons_off_and_in_place() {
    let renderer = renderer();
    let mut app = scenario_app("ring-dm");
    with_settings(&mut app, DENSITIES[0], RoomListChoice::Expanded);
    let before = Laid::new(&app, &renderer, WIDE).node.bounds();
    for ring in &mut app.calls.rings {
        ring.answered = true;
    }
    let after = Laid::new(&app, &renderer, WIDE).node.bounds();
    assert_eq!(before, after);
}

/// The absolute top-left of `target` below `node` (whose own top-left is
/// `at`), found by identity.
fn origin_of(node: &Node, target: &Node, at: iced::Point) -> Option<iced::Point> {
    let b = node.bounds();
    let here = iced::Point::new(at.x + b.x, at.y + b.y);
    if std::ptr::eq(node, target) {
        return Some(here);
    }
    node.children()
        .iter()
        .find_map(|child| origin_of(child, target, here))
}

/// The centre of `target` in window coordinates.
fn centre_of(laid: &Laid<'_>, target: &Node) -> iced::Point {
    let origin =
        origin_of(&laid.node, target, iced::Point::ORIGIN).expect("the node is in the tree");
    let b = target.bounds();
    iced::Point::new(origin.x + b.width / 2.0, origin.y + b.height / 2.0)
}

/// [`click`] after the redraw that tells buttons their status.
fn settled_click(laid: &mut Laid<'_>, renderer: &Renderer, at: iced::Point) -> Vec<Message> {
    laid.settle(renderer, WIDE);
    click(laid, renderer, WIDE, at)
}

/// The ring card's Decline and Accept nodes (the row of two equal halves).
fn ring_row<'n>(laid: &'n Laid<'_>) -> &'n Node {
    let layer = laid.node.children().last().expect("the card layer");
    let mut pairs = Vec::new();
    nodes_with(layer, 2, &mut pairs);
    pairs
        .into_iter()
        .rfind(|n| {
            let (a, b) = (n.children()[0].bounds(), n.children()[1].bounds());
            (a.y - b.y).abs() < 0.5
                && (a.width - b.width).abs() < 1.0
                && a.width > 100.0
                && a.height < 80.0
                && a.height > 20.0
        })
        .expect("the Decline and Accept row")
}

fn ring_clicks(app: &App, renderer: &Renderer) -> (Vec<Message>, Vec<Message>, Vec<Message>) {
    let mut laid = Laid::new(app, renderer, WIDE);
    let row = ring_row(&laid);
    let (decline, accept) = (
        centre_of(&laid, &row.children()[0]),
        centre_of(&laid, &row.children()[1]),
    );
    let between = iced::Point::new(WIDE.width / 2.0, accept.y);
    let a = settled_click(&mut laid, renderer, accept);
    let d = settled_click(&mut laid, renderer, decline);
    let m = settled_click(&mut laid, renderer, between);
    (a, d, m)
}

#[test]
fn a_click_on_a_ring_card_that_just_showed_answers_nothing_and_a_later_one_does() {
    use twicecat_app::calls::CallMessage;
    let renderer = renderer();
    let mut app = scenario_app("ring-dm");
    with_settings(&mut app, DENSITIES[0], RoomListChoice::Expanded);
    assert!(app.calls.arming(), "the card has just shown");
    let (accept, decline, centre) = ring_clicks(&app, &renderer);
    assert!(accept.is_empty(), "Accept while arming: {}", accept.len());
    assert!(
        decline.is_empty(),
        "Decline while arming: {}",
        decline.len()
    );
    assert!(centre.is_empty());
    // The hint says why the buttons are off.
    let seq = app.calls.arm_seq();
    let _ = app.update(Message::RingArmed(seq));
    assert!(!app.calls.arming());
    let (accept, decline, centre) = ring_clicks(&app, &renderer);
    assert!(
        matches!(accept.as_slice(), [Message::Call(CallMessage::Accept(_))]),
        "Accept once armed"
    );
    assert!(
        matches!(decline.as_slice(), [Message::Call(CallMessage::Decline(_))]),
        "Decline once armed"
    );
    // The window's centre (where a click in flight lands) is the gap
    // between the buttons: neither.
    assert!(centre.is_empty(), "the gap answers nothing");
}

#[test]
fn the_ring_card_puts_the_window_centre_in_a_wide_gap_with_accept_on_the_right() {
    let renderer = renderer();
    for size in SIZES {
        for name in ["ring-dm", "ring-group", "ring-long-id"] {
            let mut app = scenario_app(name);
            with_settings(&mut app, DENSITIES[0], RoomListChoice::Expanded);
            app.window_width = size.width;
            let laid = Laid::new(&app, &renderer, size);
            let row = ring_row(&laid);
            let decline = centre_of(&laid, &row.children()[0]);
            let accept = centre_of(&laid, &row.children()[1]);
            let (dw, aw) = (
                row.children()[0].bounds().width,
                row.children()[1].bounds().width,
            );
            let gap_from = decline.x + dw / 2.0;
            let gap_to = accept.x - aw / 2.0;
            assert!(gap_to - gap_from >= 16.0, "{name} {size:?}: gap");
            let centre = size.width / 2.0;
            assert!(
                centre > gap_from && centre < gap_to,
                "{name} {size:?}: the window centre {centre} is in the gap {gap_from}..{gap_to}"
            );
            assert!(decline.x < accept.x);
        }
    }
}

#[test]
fn clicking_the_notice_mark_or_text_never_dismisses_it_but_its_close_button_does() {
    use twicecat_app::calls::CallMessage;
    let renderer = renderer();
    for list in [RoomListChoice::Expanded, RoomListChoice::Rail] {
        let mut app = scenario_app("main-call-notice");
        with_settings(&mut app, DENSITIES[0], list);
        assert!(app.calls.call.is_none() && app.calls.notice.is_some());
        let mut laid = Laid::new(&app, &renderer, WIDE);
        let dock = dock_slot(&laid);
        let (words, close) = match list {
            // The banner: a container around [row of icon and words, row of
            // buttons].
            RoomListChoice::Expanded => {
                let column = &dock.children()[0].children()[0];
                (&column.children()[0], &column.children()[1].children()[0])
            }
            // The rail: a container around [warning mark, close button].
            RoomListChoice::Rail => {
                let column = &dock.children()[0];
                (&column.children()[0], &column.children()[1])
            }
        };
        let (words_at, close_at) = (centre_of(&laid, words), centre_of(&laid, close));
        let on_words = settled_click(&mut laid, &renderer, words_at);
        assert!(
            on_words.is_empty(),
            "{list:?}: the notice's mark took a click: {on_words:?}"
        );
        let on_close = settled_click(&mut laid, &renderer, close_at);
        assert!(
            matches!(
                on_close.as_slice(),
                [Message::Call(CallMessage::DismissNotice)]
            ),
            "{list:?}: {on_close:?}"
        );
    }
}

#[test]
fn leave_sends_once_and_is_off_while_a_leave_or_join_waits() {
    use twicecat_app::calls::{CallMessage, Pending};
    let renderer = renderer();
    for (pending, sends) in [(None, true), (Some(Pending::Leave), false)] {
        let mut app = scenario_app("main-call-connected");
        with_settings(&mut app, DENSITIES[0], RoomListChoice::Expanded);
        app.calls.pending = pending.clone();
        let mut laid = Laid::new(&app, &renderer, WIDE);
        let leave = dock_lines(dock_slot(&laid))
            .last()
            .expect("controls")
            .children()
            .last()
            .expect("Leave");
        let at = centre_of(&laid, leave);
        let messages = settled_click(&mut laid, &renderer, at);
        if sends {
            assert!(
                matches!(messages.as_slice(), [Message::Call(CallMessage::Leave)]),
                "{messages:?}"
            );
        } else {
            assert!(messages.is_empty(), "{pending:?}: {messages:?}");
        }
    }
}

// ---- Task 12: the Voice & audio card as pages --------------------------------

mod audio {
    use super::*;
    use twicecat_app::audio_settings::{AudioMessage, AudioPage};
    use twicecat_app::screens::audio_settings::NAV_WIDTH;
    use twicecat_core::api::{AudioSettings, CoreEvent, InputMode};

    /// The first node with a child of exactly `width` (the page list).
    fn find_body(node: &Node) -> Option<&Node> {
        if node.children().len() == 3 && node.children()[0].bounds().width == NAV_WIDTH {
            return Some(node);
        }
        node.children().iter().find_map(find_body)
    }

    /// The card: the nearest ancestor of the body that is the modal's card.
    fn card_of<'n>(root: &'n Node, body: &'n Node) -> &'n Node {
        fn chain<'n>(node: &'n Node, target: &'n Node, path: &mut Vec<&'n Node>) -> bool {
            path.push(node);
            if std::ptr::eq(node, target) {
                return true;
            }
            if node.children().iter().any(|c| chain(c, target, path)) {
                return true;
            }
            path.pop();
            false
        }
        let mut path = Vec::new();
        assert!(chain(root, body, &mut path), "the body is in the tree");
        // [.., card (padding 20), content (fixed height), column, body]
        path[path.len() - 4]
    }

    fn set_settings(app: &mut App, change: impl FnOnce(&mut AudioSettings)) {
        let mut settings = app.calls.settings.clone().expect("settings");
        change(&mut settings);
        let _ = app.update(Message::Core(Box::new(CoreEvent::AudioSettings(settings))));
    }

    fn page_app(name: &str) -> App {
        let mut app = scenario_app(name);
        app.set_ui_settings(variants(false)[0].settings());
        app.window_width = WIDE.width;
        app
    }

    /// Nodes that run out of their parent's width (a vertical overflow is a
    /// scroll).
    fn hspills(node: &Node, origin: iced::Point, out: &mut Vec<Rectangle>) {
        let b = node.bounds();
        let at = iced::Point::new(origin.x + b.x, origin.y + b.y);
        for child in node.children() {
            let cb = child.bounds();
            if cb.x < -0.5 || cb.x + cb.width > b.width + 0.5 {
                out.push(Rectangle::new(
                    iced::Point::new(at.x + cb.x, at.y + cb.y),
                    cb.size(),
                ));
            }
            hspills(child, at, out);
        }
    }

    const PAGE_SCENARIOS: [&str; 16] = [
        "audio-devices",
        "audio-talking",
        "audio-talking-capturing",
        "audio-talking-voice-activity",
        "audio-microphone-gpu-found",
        "audio-microphone-gpu-none",
        "audio-microphone-gpu-looking",
        "audio-microphone-gpu-fallback",
        "audio-levels",
        "audio-levels-loud",
        "audio-mic-test",
        "audio-ringtone",
        "audio-other-voices",
        "audio-announce",
        "audio-diagnostics",
        "audio-diagnostics-dropouts",
    ];

    #[test]
    fn the_card_is_760_by_640_with_the_page_list_at_its_left() {
        let renderer = renderer();
        for name in PAGE_SCENARIOS {
            for density in DENSITIES {
                let mut app = page_app(name);
                with_settings(&mut app, density, RoomListChoice::Expanded);
                let laid = Laid::new(&app, &renderer, WIDE);
                let body = find_body(&laid.node).unwrap_or_else(|| panic!("{name}: no body"));
                let card = card_of(&laid.node, body);
                let what = format!("{name} {}", density.1);
                assert_eq!(card.bounds().width, 760.0, "{what}");
                assert_eq!(card.bounds().height, 640.0, "{what}");
                // The list is at the left, the page at the right, in one row.
                let (nav, page) = (body.children()[0].bounds(), body.children()[2].bounds());
                assert!(nav.x < page.x, "{what}");
                assert!((nav.y - page.y).abs() < 0.5, "{what}");
            }
        }
    }

    #[test]
    fn a_small_window_shrinks_the_card_to_fit() {
        let renderer = renderer();
        let size = Size::new(1000.0, 420.0);
        let mut app = page_app("audio-talking");
        app.window_width = size.width;
        let laid = Laid::new(&app, &renderer, size);
        let body = find_body(&laid.node).expect("the body");
        let card = card_of(&laid.node, body);
        assert!(card.bounds().height <= size.height, "{:?}", card.bounds());
        assert!(card.bounds().height >= 280.0, "{:?}", card.bounds());
    }

    #[test]
    fn nothing_runs_out_of_the_card_sideways_in_any_page_at_both_widths() {
        let renderer = renderer();
        for name in PAGE_SCENARIOS
            .into_iter()
            .chain(["audio-devices-long-names", "audio-talking-long-key"])
        {
            for size in SIZES {
                for density in DENSITIES {
                    let mut app = page_app(name);
                    with_settings(&mut app, density, RoomListChoice::Expanded);
                    app.window_width = size.width;
                    let laid = Laid::new(&app, &renderer, size);
                    let body = find_body(&laid.node).expect("the body");
                    let card = card_of(&laid.node, body);
                    let mut out = Vec::new();
                    hspills(card, iced::Point::ORIGIN, &mut out);
                    assert!(
                        out.is_empty(),
                        "{name} {} at {}: {out:?}",
                        density.1,
                        size.width
                    );
                    assert!(
                        card.bounds().width <= size.width,
                        "{name}: the card is wider than the window"
                    );
                }
            }
        }
    }

    #[test]
    fn the_page_never_runs_under_its_scrollbar() {
        let renderer = renderer();
        let mut tall = 0;
        for name in PAGE_SCENARIOS {
            for density in DENSITIES {
                let mut app = page_app(name);
                with_settings(&mut app, density, RoomListChoice::Expanded);
                let laid = Laid::new(&app, &renderer, WIDE);
                let body = find_body(&laid.node).expect("the body");
                let scroll = &body.children()[2];
                let content = scroll.children()[0].bounds();
                // A page that fits has no scrollbar. One that does not has
                // iced's 10 px scrollbar beside it, in room of its own.
                if content.height > scroll.bounds().height + 0.5 {
                    tall += 1;
                    assert!(
                        content.x + content.width <= scroll.bounds().width - 10.0 + 0.5,
                        "{name}: content {content:?} in a scrollable {:?}",
                        scroll.bounds()
                    );
                }
            }
        }
        assert!(tall >= 4, "only {tall} tall pages: the check did not run");
    }

    #[test]
    fn the_key_row_keeps_its_size_and_its_buttons_apart_from_the_words() {
        let renderer = renderer();
        let measure = |name: &str| {
            let app = page_app(name);
            let laid = Laid::new(&app, &renderer, WIDE);
            let body = find_body(&laid.node).expect("the body");
            let page = &body.children()[2].children()[0];
            // The page column: [heading, send, note, key column, ...]
            let key = &page.children()[3];
            let row = key.children()[0].children()[1].children()[0].clone();
            (key.bounds(), row.children().len(), row)
        };
        let (idle, idle_kids, idle_row) = measure("audio-talking");
        let (waiting, waiting_kids, waiting_row) = measure("audio-talking-capturing");
        // [box, Set key / Cancel, Clear]: the same three widgets.
        assert_eq!(idle_kids, 3);
        assert_eq!(waiting_kids, 3);
        for row in [&idle_row, &waiting_row] {
            let kids = row.children();
            for pair in kids.windows(2) {
                let (a, b) = (pair[0].bounds(), pair[1].bounds());
                assert!(
                    a.x + a.width + 7.5 <= b.x,
                    "words and buttons overlap or touch: {a:?} {b:?}"
                );
            }
            let last = kids[2].bounds();
            assert!(last.x + last.width <= row.bounds().width + 0.5);
        }
        // The box is the same height with and without the capture, so
        // nothing jumps when it starts (its width is what the buttons leave);
        // the hint slot adds a line below.
        let (a, b) = (
            idle_row.children()[0].bounds(),
            waiting_row.children()[0].bounds(),
        );
        assert!((a.height - b.height).abs() < 0.5, "box {a:?} then {b:?}");
        assert!(waiting.height > idle.height, "the hint appears");
    }

    /// Every message a click at the centre of each node of the card sends,
    /// first with the page at the top, then scrolled to its end.
    fn clicked(app: &App, renderer: &Renderer) -> Vec<AudioMessage> {
        let mut laid = Laid::new(app, renderer, WIDE);
        laid.settle(renderer, WIDE);
        let (points, bottom, scroll_at) = {
            let body = find_body(&laid.node).expect("the body");
            let card = card_of(&laid.node, body);
            let origin = origin_of(&laid.node, card, iced::Point::ORIGIN).expect("card origin");
            let origin = iced::Point::new(origin.x - card.bounds().x, origin.y - card.bounds().y);
            let mut all = Vec::new();
            collect_bounds(card, origin, &mut all, 0);
            let scroll = &body.children()[2];
            let scroll_origin = origin_of(&laid.node, scroll, iced::Point::ORIGIN).expect("origin");
            let view = Rectangle::new(scroll_origin, scroll.bounds().size());
            let hidden = (scroll.children()[0].bounds().height - view.height).max(0.0);
            let mut inside = Vec::new();
            collect_bounds(&scroll.children()[0], scroll_origin, &mut inside, 0);
            // Nodes of the page, as they sit when scrolled to the end.
            let bottom: Vec<Rectangle> = inside
                .into_iter()
                .map(|(r, _)| Rectangle {
                    y: r.y - hidden,
                    ..r
                })
                .filter(|r| r.y + r.height > view.y && r.y < view.y + view.height)
                .collect();
            let points: Vec<Rectangle> = all.into_iter().map(|(r, _)| r).collect();
            (points, bottom, view.center())
        };
        let mut sent = Vec::new();
        let mut sweep = |laid: &mut Laid<'_>, rects: &[Rectangle]| {
            for rect in rects {
                if rect.width <= 0.0 || rect.height <= 0.0 {
                    continue;
                }
                for at in [
                    rect.center(),
                    iced::Point::new(rect.x + 4.0, rect.center().y),
                    iced::Point::new(rect.x + rect.width - 4.0, rect.center().y),
                ] {
                    for message in click(laid, renderer, WIDE, at) {
                        if let Message::Audio(m) = message {
                            sent.push(m);
                        }
                    }
                }
            }
        };
        sweep(&mut laid, &points);
        let wheel = Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels {
                x: 0.0,
                y: -10_000.0,
            },
        });
        let mut ignored: Vec<Message> = Vec::new();
        let mut shell = Shell::new(&mut ignored);
        laid.element.as_widget_mut().update(
            &mut laid.tree,
            &wheel,
            Layout::new(&laid.node),
            mouse::Cursor::Available(scroll_at),
            renderer,
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::with_size(WIDE),
        );
        sweep(&mut laid, &bottom);
        sent
    }

    /// The pages and the messages their controls send are the same as the
    /// table of `AudioPage::sends`, and the pages together send every message
    /// that a click can send (the menus open an overlay this harness does not
    /// drive, and a captured key comes from a key press).
    #[test]
    fn each_page_sends_only_its_own_messages_and_together_they_send_them_all() {
        use std::collections::BTreeSet;
        let renderer = renderer();
        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        // One app per page, plus the states that show the other controls.
        let mut apps: Vec<(AudioPage, String, App)> = Vec::new();
        for (page, name) in [
            (AudioPage::Devices, "audio-devices"),
            (AudioPage::Talking, "audio-talking"),
            (AudioPage::Talking, "audio-talking-capturing"),
            (AudioPage::Talking, "audio-talking-voice-activity"),
            (AudioPage::Microphone, "audio-microphone-gpu-found"),
            (AudioPage::Levels, "audio-levels"),
            (AudioPage::MicTest, "audio-mic-test"),
            (AudioPage::Ringtone, "audio-ringtone"),
            (AudioPage::OtherVoices, "audio-other-voices"),
            (AudioPage::Announce, "audio-announce"),
            (AudioPage::Diagnostics, "audio-diagnostics"),
        ] {
            apps.push((page, name.to_owned(), page_app(name)));
        }
        let mut gate = page_app("audio-microphone-gpu-found");
        set_settings(&mut gate, |s| {
            s.input_mode = InputMode::VoiceActivity {
                automatic: false,
                threshold_db: -40.0,
            };
            s.processing.echo_cancellation = true;
            s.processing.auto_gain = false;
        });
        apps.push((AudioPage::Microphone, "gate".to_owned(), gate));
        let mut announce = page_app("audio-announce");
        set_settings(&mut announce, |s| s.announce_cleaning.enabled = true);
        apps.push((AudioPage::Announce, "announce on".to_owned(), announce));
        let mut quiet_mic = page_app("audio-mic-test");
        let _ = quiet_mic.update(Message::Audio(AudioMessage::MicTest));
        apps.push((AudioPage::MicTest, "test off".to_owned(), quiet_mic));
        for (page, name, app) in &apps {
            for message in clicked(app, &renderer) {
                let allowed = AudioMessage::CARD_WIDE.contains(&message.name())
                    || page.sends().contains(&message.name());
                assert!(allowed, "{name}: {message:?} is not a message of {page:?}");
                if let Some(owner) = message.page() {
                    assert_eq!(owner, *page, "{name}: {message:?}");
                }
                seen.insert(message.name());
            }
        }
        let unreachable_by_click = [
            // Menus: their choices come from the overlay.
            "InputDevice",
            "OutputDevice",
            "Noise",
            "Level",
            "Bitrate",
            "Skip",
            "Mode",
            "Button",
            // A key press, and a timer.
            "Captured",
            "SlideDue",
            // Only the talk line opens a page directly.
            "OpenAt",
            "Open",
        ];
        let mut expected: BTreeSet<&str> = AudioMessage::CARD_WIDE.into_iter().collect();
        for page in AudioPage::ALL {
            expected.extend(page.sends());
        }
        for name in unreachable_by_click {
            expected.remove(name);
        }
        let missing: Vec<&&str> = expected.iter().filter(|n| !seen.contains(**n)).collect();
        assert!(
            missing.is_empty(),
            "no click sent {missing:?}; saw {seen:?}"
        );
    }

    #[test]
    fn the_talk_line_in_the_dock_opens_the_talking_page() {
        let renderer = renderer();
        let mut app = scenario_app("main-call-ptt-idle");
        with_settings(&mut app, DENSITIES[0], RoomListChoice::Expanded);
        let mut laid = Laid::new(&app, &renderer, WIDE);
        laid.settle(&renderer, WIDE);
        let (lines, audio) = {
            let dock = dock_slot(&laid);
            let lines: Vec<iced::Point> = dock_lines(dock)
                .iter()
                .map(|line| centre_of(&laid, line))
                .collect();
            let controls = dock_lines(dock).last().expect("controls");
            (lines, centre_of(&laid, &controls.children()[2]))
        };
        let mut hit = false;
        for at in lines {
            hit |= click(&mut laid, &renderer, WIDE, at)
                .iter()
                .any(|m| matches!(m, Message::Audio(AudioMessage::OpenAt(AudioPage::Talking))));
        }
        assert!(hit, "no line of the dock opens the Talking page");
        // The audio button still opens Devices.
        let sent = click(&mut laid, &renderer, WIDE, audio);
        assert!(
            sent.iter()
                .any(|m| matches!(m, Message::Audio(AudioMessage::Open))),
            "{sent:?}"
        );
    }

    #[test]
    fn the_status_lines_stay_when_the_page_changes_and_a_notice_has_its_slot() {
        let renderer = renderer();
        let mut heights = Vec::new();
        for name in ["audio-devices", "audio-levels", "audio-diagnostics"] {
            let app = page_app(name);
            let laid = Laid::new(&app, &renderer, WIDE);
            let body = find_body(&laid.node).expect("the body");
            let card = card_of(&laid.node, body);
            // [card padding] content > column [top, body]; the top is
            // [header, status, notice slot].
            let top = &card.children()[0].children()[0].children()[0];
            assert_eq!(top.children().len(), 3, "{name}: header, status, notice");
            heights.push(top.bounds().height);
        }
        assert!(
            heights.windows(2).all(|w| (w[0] - w[1]).abs() < 0.5),
            "{heights:?}"
        );
    }
}

// ---- Task 13: login, browser sign-in and loading ---------------------------

mod login {
    use iced::advanced::widget::{Id, Operation, operation::Focusable};
    use iced::keyboard::{self, Key, key};
    use twicecat_app::screens::login::{
        CARD_WIDTH, LoginStop, PASSWORD_ID, SERVER_ID, USER_ID, focus_order,
    };

    use super::*;

    /// The focusable widgets with an id, in tree order, with their bounds.
    struct Stops(Vec<(String, Rectangle)>);

    impl Operation for Stops {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _state: &mut dyn Focusable) {
            self.0.push((format!("{id:?}"), bounds));
        }
    }

    fn stops(laid: &mut Laid<'_>, renderer: &Renderer) -> Vec<(String, Rectangle)> {
        let mut op = Stops(Vec::new());
        laid.element.as_widget_mut().operate(
            &mut laid.tree,
            Layout::new(&laid.node),
            renderer,
            &mut op,
        );
        op.0
    }

    fn id_of(name: &'static str) -> String {
        format!("{:?}", Some(Id::new(name)))
    }

    fn app_of(name: &str, width: f32) -> App {
        let mut app = scenario_app(name);
        app.window_width = width;
        app
    }

    #[test]
    fn tab_walks_the_inputs_in_the_order_of_the_focus_function() {
        let renderer = renderer();
        for (scenario, open) in [("login", false), ("login-server-open", true)] {
            let app = app_of(scenario, WIDE.width);
            let mut laid = Laid::new(&app, &renderer, WIDE);
            let found = stops(&mut laid, &renderer);
            let wanted: Vec<String> = focus_order(open)
                .into_iter()
                .filter(|stop| stop.is_input())
                .map(|stop| match stop {
                    LoginStop::User => id_of(USER_ID),
                    LoginStop::Password => id_of(PASSWORD_ID),
                    _ => id_of(SERVER_ID),
                })
                .collect();
            let ids: Vec<&String> = found.iter().map(|(id, _)| id).collect();
            assert_eq!(ids, wanted.iter().collect::<Vec<_>>(), "{scenario}");
            let tops: Vec<f32> = found.iter().map(|(_, b)| b.y).collect();
            assert!(tops.windows(2).all(|w| w[0] < w[1]), "{scenario}: {tops:?}");
        }
    }

    #[test]
    fn the_card_is_420_wide_and_centred_and_nothing_runs_out_of_it() {
        let renderer = renderer();
        let names = [
            "login",
            "login-server",
            "login-server-open",
            "login-server-notice",
            "login-busy",
            "login-error",
            "waiting",
            "waiting-opening",
            "waiting-failed",
            "loading",
            "loading-reconnecting",
        ];
        for name in names {
            for size in [WIDE, NARROW, Size::new(360.0, 640.0)] {
                let app = app_of(name, size.width);
                let laid = Laid::new(&app, &renderer, size);
                let mut all = Vec::new();
                collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
                let expect = CARD_WIDTH.min(size.width - 2.0 * 16.0);
                let card = all
                    .iter()
                    .find(|(r, d)| {
                        *d >= 2 && r.width < size.width - 0.5 && (r.width - expect).abs() < 0.5
                    })
                    .unwrap_or_else(|| panic!("{name} at {}: no card of {expect}", size.width))
                    .0;
                assert!(
                    (card.center_x() - size.width / 2.0).abs() < 0.5,
                    "{name} is centred"
                );
                let mut out = Vec::new();
                spills(&laid.node, iced::Point::ORIGIN, &mut out);
                assert!(out.is_empty(), "{name} at {}: {out:?}", size.width);
                assert!(card.y >= 0.0 && card.y + card.height <= size.height + 0.5);
            }
        }
    }

    #[test]
    fn a_long_server_shows_whole_on_lines_of_its_own_under_the_field() {
        let renderer = renderer();
        for size in [WIDE, NARROW, Size::new(360.0, 640.0)] {
            let app = app_of("login-server", size.width);
            let mut laid = Laid::new(&app, &renderer, size);
            let fields = stops(&mut laid, &renderer);
            let server = fields
                .iter()
                .find(|(id, _)| *id == id_of(SERVER_ID))
                .expect("the server field is open")
                .1;
            // The echo is the next text under the field: several lines
            // tall, inside the card, and starting below the field.
            let mut all = Vec::new();
            collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
            let echo = all
                .iter()
                .map(|(r, _)| r)
                .find(|r| {
                    r.y >= server.y + server.height - 0.5
                        && r.y < server.y + server.height + 12.0
                        && r.height > 2.0 * 12.0
                        && r.width >= server.width - 1.0
                })
                .unwrap_or_else(|| panic!("a wrapped echo under the field at {}", size.width));
            assert!(echo.x >= server.x - 0.5);
            assert!(echo.x + echo.width <= server.x + server.width + 0.5);
        }
    }

    fn enter() -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(key::Named::Enter),
            modified_key: Key::Named(key::Named::Enter),
            physical_key: key::Physical::Code(key::Code::Enter),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        })
    }

    /// Clicks a field to focus it, presses Enter, and returns what came out
    /// of the Enter.
    fn enter_in(app: &App, renderer: &Renderer, which: &'static str) -> Vec<Message> {
        let mut laid = Laid::new(app, renderer, WIDE);
        laid.settle(renderer, WIDE);
        let field = stops(&mut laid, renderer)
            .into_iter()
            .find(|(id, _)| *id == id_of(which))
            .map(|(_, b)| b);
        let Some(field) = field else {
            return Vec::new();
        };
        let _ = click(&mut laid, renderer, WIDE, field.center());
        let mut messages: Vec<Message> = Vec::new();
        let mut shell = Shell::new(&mut messages);
        laid.element.as_widget_mut().update(
            &mut laid.tree,
            &enter(),
            Layout::new(&laid.node),
            mouse::Cursor::Available(field.center()),
            renderer,
            &mut clipboard::Null,
            &mut shell,
            &Rectangle::with_size(WIDE),
        );
        messages
    }

    fn filled() -> App {
        let mut app = scenario_app("login-server-open");
        for message in [
            Message::UsernameChanged("alice".into()),
            Message::PasswordChanged(twicecat_core::api::Password::from("gallery-filler")),
        ] {
            let _ = app.update(message);
        }
        app
    }

    #[test]
    fn enter_submits_from_every_field_only_when_the_form_is_complete() {
        let renderer = renderer();
        let ready = filled();
        for field in [USER_ID, PASSWORD_ID, SERVER_ID] {
            let sent = enter_in(&ready, &renderer, field);
            assert!(
                sent.iter().any(|m| matches!(m, Message::LoginSubmitted)),
                "Enter in {field}: {sent:?}"
            );
        }
        // No password, or only blanks as the user: Enter does nothing.
        let mut no_password = filled();
        let _ = no_password.update(Message::PasswordChanged(
            twicecat_core::api::Password::from(String::new()),
        ));
        let mut no_user = filled();
        let _ = no_user.update(Message::UsernameChanged("  ".into()));
        for app in [&no_password, &no_user] {
            for field in [USER_ID, PASSWORD_ID, SERVER_ID] {
                assert!(enter_in(app, &renderer, field).is_empty(), "{field}");
            }
        }
    }

    #[test]
    fn enter_does_nothing_while_the_server_hides_behind_userinfo() {
        let renderer = renderer();
        let mut app = filled();
        let hidden = format!("https://matrix.org{}@evil.example", "a".repeat(300));
        let _ = app.update(Message::ServerChanged(hidden));
        for field in [USER_ID, PASSWORD_ID, SERVER_ID] {
            let sent = enter_in(&app, &renderer, field);
            assert!(sent.is_empty(), "Enter in {field}: {sent:?}");
        }
    }

    #[test]
    fn while_signing_in_the_fields_and_every_button_do_nothing() {
        let renderer = renderer();
        let mut app = filled();
        let _ = app.update(Message::LoginSubmitted);
        assert!(app.login_in_flight);
        for field in [USER_ID, PASSWORD_ID, SERVER_ID] {
            assert!(enter_in(&app, &renderer, field).is_empty(), "{field}");
        }
        let mut laid = Laid::new(&app, &renderer, WIDE);
        laid.settle(&renderer, WIDE);
        let mut all = Vec::new();
        collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
        for (rect, _) in all.iter().filter(|(r, _)| r.width > 0.0 && r.height > 0.0) {
            let sent = click(&mut laid, &renderer, WIDE, rect.center());
            assert!(sent.is_empty(), "a click at {rect:?} sent {sent:?}");
        }
    }

    #[test]
    fn an_idle_card_has_a_log_in_a_browser_and_a_server_button_that_each_send_one_message() {
        let renderer = renderer();
        let app = scenario_app("login");
        let mut seen = [false; 3];
        let mut laid = Laid::new(&app, &renderer, WIDE);
        laid.settle(&renderer, WIDE);
        let mut all = Vec::new();
        collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
        for (rect, _) in all.iter().filter(|(r, _)| r.width > 0.0 && r.height > 0.0) {
            for message in click(&mut laid, &renderer, WIDE, rect.center()) {
                match message {
                    Message::LoginSubmitted => seen[0] = true,
                    Message::OAuthRequested => seen[1] = true,
                    Message::ToggleServerField => seen[2] = true,
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        assert_eq!(seen, [true; 3]);
    }

    #[test]
    fn the_disclosure_button_is_off_while_the_field_is_held_open() {
        let renderer = renderer();
        for name in ["login-server", "login-server-notice"] {
            let app = scenario_app(name);
            let mut laid = Laid::new(&app, &renderer, WIDE);
            laid.settle(&renderer, WIDE);
            let mut all = Vec::new();
            collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
            for (rect, _) in all.iter().filter(|(r, _)| r.width > 0.0 && r.height > 0.0) {
                let sent = click(&mut laid, &renderer, WIDE, rect.center());
                assert!(
                    !sent.iter().any(|m| matches!(m, Message::ToggleServerField)),
                    "{name}: a click at {rect:?} sent {sent:?}"
                );
            }
        }
    }

    #[test]
    fn the_disclosure_opens_on_a_click_and_stays_open_for_a_server_or_a_server_notice() {
        let mut app = scenario_app("login");
        assert!(!app.login.server_open);
        let _ = app.update(Message::ToggleServerField);
        assert!(app.login.server_open);
        let _ = app.update(Message::ToggleServerField);
        assert!(!app.login.server_open);
        // With a server typed, the field stays and the button does nothing.
        let renderer = renderer();
        let mut held = scenario_app("login-server");
        assert!(!held.login.server_open);
        let _ = held.update(Message::ToggleServerField);
        assert!(!held.login.server_open);
        let mut laid = Laid::new(&held, &renderer, WIDE);
        assert!(
            stops(&mut laid, &renderer)
                .iter()
                .any(|(id, _)| *id == id_of(SERVER_ID))
        );
        // The notice about the server opens the empty field on its own.
        let notice = scenario_app("login-server-notice");
        let mut laid = Laid::new(&notice, &renderer, WIDE);
        assert!(
            stops(&mut laid, &renderer)
                .iter()
                .any(|(id, _)| *id == id_of(SERVER_ID))
        );
    }

    #[test]
    fn the_waiting_card_offers_copy_link_only_after_a_failed_launch_and_never_shows_the_url() {
        let renderer = renderer();
        let sent = |name: &str| {
            let app = scenario_app(name);
            let mut laid = Laid::new(&app, &renderer, WIDE);
            laid.settle(&renderer, WIDE);
            let mut all = Vec::new();
            collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
            let mut out = Vec::new();
            for (rect, _) in all.iter().filter(|(r, _)| r.width > 0.0 && r.height > 0.0) {
                out.extend(click(&mut laid, &renderer, WIDE, rect.center()));
            }
            out
        };
        for name in ["waiting", "waiting-opening"] {
            let messages = sent(name);
            assert!(messages.iter().any(|m| matches!(m, Message::CancelOAuth)));
            assert!(!messages.iter().any(|m| matches!(m, Message::CopyLink)));
        }
        let failed = sent("waiting-failed");
        assert!(failed.iter().any(|m| matches!(m, Message::CancelOAuth)));
        assert!(failed.iter().any(|m| matches!(m, Message::CopyLink)));
    }

    /// The smaller of two rectangles: the button, not the row around it.
    fn smaller(old: Option<Rectangle>, new: Rectangle) -> Option<Rectangle> {
        match old {
            Some(old) if old.width * old.height <= new.width * new.height => Some(old),
            _ => Some(new),
        }
    }

    #[test]
    fn the_waiting_buttons_sit_on_a_row_under_the_words() {
        let renderer = renderer();
        for size in [WIDE, Size::new(360.0, 640.0)] {
            let app = app_of("waiting-failed", size.width);
            let mut laid = Laid::new(&app, &renderer, size);
            laid.settle(&renderer, size);
            let mut all = Vec::new();
            collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
            let (mut copy, mut cancel) = (None, None);
            for (rect, _) in all.iter().filter(|(r, _)| r.width > 0.0 && r.height > 0.0) {
                for message in click(&mut laid, &renderer, size, rect.center()) {
                    match message {
                        Message::CopyLink => copy = smaller(copy, *rect),
                        Message::CancelOAuth => cancel = smaller(cancel, *rect),
                        _ => {}
                    }
                }
            }
            let (copy, cancel) = (copy.expect("Copy link"), cancel.expect("Cancel"));
            assert!(cancel.x >= copy.x + copy.width, "side by side, apart");
            // Every other node ends above the buttons' row.
            let row_top = copy.y.min(cancel.y);
            let text_bottom = all
                .iter()
                .map(|(r, _)| r)
                .filter(|r| r.height > 12.0 && r.height < 120.0 && r.width > copy.width + 20.0)
                .filter(|r| r.y + r.height <= row_top + 0.5)
                .count();
            assert!(
                text_bottom > 0,
                "the words end above the buttons at {}",
                size.width
            );
        }
    }
}

// ---- Task 14: Settings and Sessions as pages -----------------------------

/// What the `Operation::text` of every text widget under a screen says, with
/// the bounds the widget was laid out in.
struct Texts(Vec<(String, Rectangle)>);

impl iced::advanced::widget::Operation for Texts {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
        operate(self);
    }

    fn text(&mut self, _id: Option<&iced::advanced::widget::Id>, bounds: Rectangle, text: &str) {
        self.0.push((text.to_owned(), bounds));
    }
}

fn texts(laid: &mut Laid<'_>, renderer: &Renderer) -> Vec<(String, Rectangle)> {
    let mut op = Texts(Vec::new());
    laid.element.as_widget_mut().operate(
        &mut laid.tree,
        Layout::new(&laid.node),
        renderer,
        &mut op,
    );
    op.0
}

/// Nodes that run out of their parent's width (a vertical overflow is a
/// scroll).
fn hspills(node: &Node, origin: iced::Point, out: &mut Vec<Rectangle>) {
    let b = node.bounds();
    let at = iced::Point::new(origin.x + b.x, origin.y + b.y);
    for child in node.children() {
        let cb = child.bounds();
        if cb.x < -0.5 || cb.x + cb.width > b.width + 0.5 {
            out.push(Rectangle::new(
                iced::Point::new(at.x + cb.x, at.y + cb.y),
                cb.size(),
            ));
        }
        hspills(child, at, out);
    }
}

/// The nearest ancestor of `body` that is the modal's card:
/// `[.., card (padding 20), content (fixed height), column, body]`.
fn card_above<'n>(root: &'n Node, body: &'n Node) -> &'n Node {
    fn chain<'n>(node: &'n Node, target: &'n Node, path: &mut Vec<&'n Node>) -> bool {
        path.push(node);
        if std::ptr::eq(node, target) {
            return true;
        }
        if node.children().iter().any(|c| chain(c, target, path)) {
            return true;
        }
        path.pop();
        false
    }
    let mut path = Vec::new();
    assert!(chain(root, body, &mut path), "the body is in the tree");
    path[path.len() - 4]
}

/// The distance between two rectangles (zero when they touch or overlap).
fn gap(a: Rectangle, b: Rectangle) -> f32 {
    let dx = (b.x - (a.x + a.width)).max(a.x - (b.x + b.width)).max(0.0);
    let dy = (b.y - (a.y + a.height))
        .max(a.y - (b.y + b.height))
        .max(0.0);
    dx.hypot(dy)
}

/// For every node of the screen the message(s) a click at its centre sends,
/// as `Debug` text, with the node's window bounds; only nodes that send.
fn senders(app: &App, renderer: &Renderer, size: Size) -> Vec<(Rectangle, String)> {
    let mut laid = Laid::new(app, renderer, size);
    laid.settle(renderer, size);
    let mut all = Vec::new();
    collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
    let mut out: Vec<(Rectangle, String)> = Vec::new();
    for (rect, _) in all {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            continue;
        }
        for message in click(&mut laid, renderer, size, rect.center()) {
            let text = format!("{message:?}");
            // Keep the largest node of each message (the button, not its
            // text), at each place.
            match out
                .iter_mut()
                .find(|(r, t)| *t == text && gap(*r, rect) == 0.0)
            {
                Some(slot) => {
                    if rect.width * rect.height > slot.0.width * slot.0.height {
                        slot.0 = rect;
                    }
                }
                None => out.push((rect, text)),
            }
        }
    }
    out
}

mod settings_pages {
    use std::collections::BTreeSet;

    use iced::advanced::widget::{Id, Operation, operation::Focusable};
    use twicecat_app::screens::settings::NAV_WIDTH;
    use twicecat_app::settings::{
        INTER_ATTRIBUTION, INTER_NOTICES, KEYWORD_INPUT_ID, NAME_INPUT_ID, SettingsMessage,
        SettingsPage,
    };
    use twicecat_core::api::{CoreEvent, OwnProfileView};

    use super::*;

    const PAGE_SCENARIOS: [(&str, SettingsPage); 7] = [
        ("settings", SettingsPage::Profile),
        ("settings-appearance", SettingsPage::Appearance),
        ("settings-notifications", SettingsPage::Notifications),
        ("settings-voice-audio", SettingsPage::VoiceAudio),
        ("settings-keyboard", SettingsPage::Keyboard),
        ("settings-sessions", SettingsPage::Sessions),
        ("settings-about", SettingsPage::About),
    ];

    fn find_body(node: &Node) -> Option<&Node> {
        if node.children().len() == 3 && node.children()[0].bounds().width == NAV_WIDTH {
            return Some(node);
        }
        node.children().iter().find_map(find_body)
    }

    fn page_app(name: &str) -> App {
        let mut app = scenario_app(name);
        app.set_ui_settings(variants(false)[0].settings());
        app.window_width = WIDE.width;
        app
    }

    #[test]
    fn the_card_is_760_by_620_with_the_page_list_at_its_left() {
        let renderer = renderer();
        for (name, _) in PAGE_SCENARIOS {
            for density in DENSITIES {
                let mut app = page_app(name);
                with_settings(&mut app, density, RoomListChoice::Expanded);
                let laid = Laid::new(&app, &renderer, WIDE);
                let body = find_body(&laid.node).unwrap_or_else(|| panic!("{name}: no body"));
                let card = card_above(&laid.node, body);
                let what = format!("{name} {}", density.1);
                assert_eq!(card.bounds().width, 760.0, "{what}");
                assert_eq!(card.bounds().height, 620.0, "{what}");
                let (nav, page) = (body.children()[0].bounds(), body.children()[2].bounds());
                assert!(nav.x < page.x, "{what}");
                assert!((nav.y - page.y).abs() < 0.5, "{what}");
            }
        }
    }

    #[test]
    fn a_small_window_shrinks_the_card_to_fit() {
        let renderer = renderer();
        let size = Size::new(1000.0, 420.0);
        let mut app = page_app("settings-appearance");
        app.window_width = size.width;
        let laid = Laid::new(&app, &renderer, size);
        let body = find_body(&laid.node).expect("the body");
        let card = card_above(&laid.node, body);
        assert!(card.bounds().height <= size.height, "{:?}", card.bounds());
        assert!(card.bounds().height >= 280.0, "{:?}", card.bounds());
    }

    #[test]
    fn nothing_runs_out_of_the_card_sideways_in_any_page_at_both_widths() {
        let renderer = renderer();
        let names = PAGE_SCENARIOS
            .map(|(name, _)| name)
            .into_iter()
            .chain(["settings-notifications-long"]);
        for name in names {
            for size in SIZES {
                for density in DENSITIES {
                    let mut app = page_app(name);
                    with_settings(&mut app, density, RoomListChoice::Expanded);
                    app.window_width = size.width;
                    let laid = Laid::new(&app, &renderer, size);
                    let body = find_body(&laid.node).expect("the body");
                    let card = card_above(&laid.node, body);
                    let mut out = Vec::new();
                    hspills(card, iced::Point::ORIGIN, &mut out);
                    assert!(
                        out.is_empty(),
                        "{name} {} at {}: {out:?}",
                        density.1,
                        size.width
                    );
                    assert!(card.bounds().width <= size.width, "{name}");
                }
            }
        }
    }

    #[test]
    fn the_page_never_runs_under_its_scrollbar() {
        let renderer = renderer();
        let mut tall = 0;
        for (name, _) in PAGE_SCENARIOS {
            for density in DENSITIES {
                let mut app = page_app(name);
                with_settings(&mut app, density, RoomListChoice::Expanded);
                let laid = Laid::new(&app, &renderer, WIDE);
                let body = find_body(&laid.node).expect("the body");
                let scroll = &body.children()[2];
                let content = scroll.children()[0].bounds();
                if content.height > scroll.bounds().height + 0.5 {
                    tall += 1;
                    assert!(
                        content.x + content.width <= scroll.bounds().width - 10.0 + 0.5,
                        "{name}: content {content:?} in a scrollable {:?}",
                        scroll.bounds()
                    );
                }
            }
        }
        assert!(tall >= 2, "only {tall} tall pages: the check did not run");
    }

    #[test]
    fn the_four_colour_sets_fit_the_page_without_spilling() {
        // The swatches are two by two: all four are on screen, none past the
        // page, in the narrowest card.
        let renderer = renderer();
        let mut app = page_app("settings-appearance");
        app.window_width = NARROW.width;
        let mut laid = Laid::new(&app, &renderer, NARROW);
        let shown = texts(&mut laid, &renderer);
        for label in ["Twicecat", "Frost", "Ember", "High contrast"] {
            assert!(
                shown.iter().any(|(t, _)| t.starts_with(label)),
                "no swatch named {label}"
            );
        }
    }

    #[test]
    fn a_long_keyword_wraps_and_its_end_is_never_cut_off() {
        let renderer = renderer();
        let app = page_app("settings-notifications-long");
        let mut laid = Laid::new(&app, &renderer, WIDE);
        let shown = texts(&mut laid, &renderer);
        let long = "k".repeat(64);
        let (_, bounds) = shown
            .iter()
            .find(|(t, _)| *t == long)
            .expect("the whole keyword is drawn");
        // 64 characters with no break point do not fit one line of the
        // page: the words wrap by glyph (a line of text is about 17 px).
        assert!(bounds.height > 30.0, "the keyword did not wrap: {bounds:?}");
        // The keyword is in a row with its Remove button: the words are
        // laid out in the room the button leaves, so the row cannot cover it.
        let buttons: Vec<Rectangle> = shown
            .iter()
            .filter(|(t, _)| t == "Remove")
            .map(|(_, b)| *b)
            .collect();
        assert!(!buttons.is_empty());
        for button in buttons {
            if (button.y - bounds.y).abs() < bounds.height {
                assert!(
                    bounds.x + bounds.width <= button.x + 0.5,
                    "the keyword {bounds:?} runs under Remove {button:?}"
                );
            }
        }
        // And the failure line has its slot, with its words.
        assert!(
            shown
                .iter()
                .any(|(t, _)| t == "The settings could not be saved on this device.")
        );
    }

    #[test]
    fn the_about_page_names_the_font_and_where_its_licence_is() {
        assert!(INTER_ATTRIBUTION.contains("Inter"));
        assert!(INTER_ATTRIBUTION.contains("2016 The Inter Project Authors"));
        assert!(INTER_ATTRIBUTION.contains("SIL Open Font License 1.1"));
        assert!(INTER_NOTICES.contains("OFL.txt"));
        assert!(INTER_NOTICES.contains("THIRD-PARTY-NOTICES.md"));
        // The fonts are embedded: the text must not say they have a folder
        // next to the program.
        assert!(!INTER_NOTICES.contains("next to"));
        assert!(INTER_NOTICES.contains("crates/app/assets/fonts/inter"));
        assert!(INTER_NOTICES.contains("https://openfontlicense.org"));
        let renderer = renderer();
        let app = page_app("settings-about");
        let mut laid = Laid::new(&app, &renderer, WIDE);
        let shown: Vec<String> = texts(&mut laid, &renderer)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        for wanted in [INTER_ATTRIBUTION, INTER_NOTICES] {
            assert!(shown.iter().any(|t| t == wanted), "About lacks {wanted:?}");
        }
        let version = format!("Twicecat {}", env!("CARGO_PKG_VERSION"));
        assert!(shown.contains(&version), "no version line");
        assert!(
            shown.iter().any(|t| t.contains("native Wayland")),
            "no drag-and-drop note"
        );
        // The fixed THIRD-PARTY-NOTICES.md says the same words.
        let notices = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../THIRD-PARTY-NOTICES.md"),
        )
        .expect("the notices file");
        assert!(notices.contains("Copyright (c) 2016 The Inter Project Authors"));
    }

    /// The page on show draws its own title (and the list has its name); the
    /// other pages have only their name in the list.
    #[test]
    fn each_page_shows_its_own_title_and_other_pages_do_not() {
        let renderer = renderer();
        for (name, page) in PAGE_SCENARIOS {
            let app = page_app(name);
            let mut laid = Laid::new(&app, &renderer, WIDE);
            let shown = texts(&mut laid, &renderer);
            for other in SettingsPage::ALL {
                let count = shown.iter().filter(|(t, _)| t == other.label()).count();
                let wanted = if other == page { 2 } else { 1 };
                assert_eq!(count, wanted, "{name}: {}", other.label());
            }
        }
    }

    /// Every message a click at the centre of each node of the card sends.
    fn clicked(app: &App, renderer: &Renderer) -> Vec<SettingsMessage> {
        let mut laid = Laid::new(app, renderer, WIDE);
        laid.settle(renderer, WIDE);
        let mut all = Vec::new();
        collect_bounds(&laid.node, iced::Point::ORIGIN, &mut all, 0);
        let mut sent = Vec::new();
        for (rect, _) in all {
            if rect.width <= 0.0 || rect.height <= 0.0 {
                continue;
            }
            for at in [
                rect.center(),
                iced::Point::new(rect.x + 4.0, rect.center().y),
                iced::Point::new(rect.x + rect.width - 4.0, rect.center().y),
            ] {
                for message in click(&mut laid, renderer, WIDE, at) {
                    if let Message::Settings(m) = message {
                        sent.push(m);
                    }
                }
            }
        }
        sent
    }

    fn feed(app: &mut App, event: CoreEvent) {
        let _ = app.update(Message::Core(Box::new(event)));
    }

    #[test]
    fn each_page_sends_only_its_own_messages_and_together_they_send_them_all() {
        let renderer = renderer();
        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        let mut apps: Vec<(SettingsPage, String, App)> = PAGE_SCENARIOS
            .into_iter()
            .map(|(name, page)| (page, name.to_owned(), page_app(name)))
            .collect();
        // Profile with an edited name and a picture set: Save and Remove
        // picture are on.
        let mut profile = page_app("settings");
        feed(
            &mut profile,
            CoreEvent::OwnProfile(OwnProfileView {
                display_name: Some("Me Example".to_owned()),
                avatar: Some(twicecat_core::api::AvatarId::from("mxc://example.org/a")),
            }),
        );
        let _ = profile.update(Message::Settings(SettingsMessage::NameInput(
            "Me Edited".to_owned().into(),
        )));
        apps.push((SettingsPage::Profile, "profile edited".to_owned(), profile));
        // Notifications with a typed keyword: Add is on.
        let mut typed = page_app("settings-notifications");
        let _ = typed.update(Message::Settings(SettingsMessage::KeywordInput(
            "word".to_owned().into(),
        )));
        apps.push((SettingsPage::Notifications, "typed".to_owned(), typed));
        // Appearance with a non-default colour set and a larger text: every
        // button of the row is on.
        let mut zoomed = page_app("settings-appearance");
        let _ = zoomed.update(Message::Settings(SettingsMessage::Zoom(
            twicecat_app::settings::Zoom::In,
        )));
        apps.push((SettingsPage::Appearance, "zoomed".to_owned(), zoomed));
        for (page, name, app) in &apps {
            for message in clicked(app, &renderer) {
                let allowed = SettingsMessage::CARD_WIDE.contains(&message.name())
                    || page.sends().contains(&message.name());
                assert!(allowed, "{name}: {message:?} is not a message of {page:?}");
                seen.insert(message.name());
            }
        }
        // Menus open an overlay this harness does not drive; typing makes
        // the text messages.
        let unreachable_by_click = [
            "DefaultMode",
            "Density",
            "RoomList",
            "NameInput",
            "KeywordInput",
        ];
        let mut expected: BTreeSet<&str> = SettingsMessage::CARD_WIDE.into_iter().collect();
        for page in SettingsPage::ALL {
            expected.extend(page.sends());
        }
        for name in unreachable_by_click {
            expected.remove(name);
        }
        let missing: Vec<&&str> = expected.iter().filter(|n| !seen.contains(**n)).collect();
        assert!(
            missing.is_empty(),
            "no click sent {missing:?}; saw {seen:?}"
        );
    }

    /// A page of the list sends the page it names.
    #[test]
    fn the_page_list_has_a_button_for_each_page() {
        let renderer = renderer();
        let app = page_app("settings");
        let mut pages = BTreeSet::new();
        for (_, text) in senders(&app, &renderer, WIDE) {
            if let Some(rest) = text.strip_prefix("Settings(Page(") {
                pages.insert(rest.trim_end_matches("))").to_owned());
            }
        }
        let wanted: BTreeSet<String> = SettingsPage::ALL.iter().map(|p| format!("{p:?}")).collect();
        assert_eq!(pages, wanted);
    }

    /// Voice & audio and Sessions each have the one button that opens their
    /// own screen, apart from the Close of the card.
    #[test]
    fn the_voice_and_sessions_pages_have_their_one_button() {
        let renderer = renderer();
        for (name, wanted) in [
            ("settings-voice-audio", "Settings(OpenVoiceAudio)"),
            ("settings-sessions", "Settings(OpenSessions)"),
        ] {
            let app = page_app(name);
            let found: Vec<String> = senders(&app, &renderer, WIDE)
                .into_iter()
                .map(|(_, t)| t)
                .filter(|t| t.starts_with("Settings(Open"))
                .collect();
            assert_eq!(found, vec![wanted.to_owned()], "{name}");
        }
    }

    struct Stops(Vec<String>);

    impl Operation for Stops {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn Focusable) {
            self.0.push(format!("{id:?}"));
        }
    }

    /// Tab walks the text fields of the page on show (this iced gives
    /// buttons no focus): the name on Profile, the keyword on Notifications,
    /// nothing named on the others.
    #[test]
    fn tab_stops_are_the_text_fields_of_the_page_on_show() {
        let renderer = renderer();
        let id_of = |name: &'static str| format!("{:?}", Some(Id::new(name)));
        for (name, page) in PAGE_SCENARIOS {
            let app = page_app(name);
            let mut laid = Laid::new(&app, &renderer, WIDE);
            let mut op = Stops(Vec::new());
            laid.element.as_widget_mut().operate(
                &mut laid.tree,
                Layout::new(&laid.node),
                &renderer,
                &mut op,
            );
            let mine: Vec<&String> = op.0.iter().filter(|id| id.contains("settings-")).collect();
            let wanted = match page {
                SettingsPage::Profile => vec![id_of(NAME_INPUT_ID)],
                SettingsPage::Notifications => vec![id_of(KEYWORD_INPUT_ID)],
                _ => Vec::new(),
            };
            assert_eq!(mine, wanted.iter().collect::<Vec<_>>(), "{name}");
        }
    }

    #[test]
    fn the_notice_slot_is_there_on_every_page_and_takes_no_room_when_empty() {
        let renderer = renderer();
        let mut heights = Vec::new();
        for (name, _) in PAGE_SCENARIOS {
            let app = page_app(name);
            let laid = Laid::new(&app, &renderer, WIDE);
            let body = find_body(&laid.node).expect("the body");
            let card = card_above(&laid.node, body);
            // content > column [top [header, notice], body]
            let top = &card.children()[0].children()[0].children()[0];
            assert_eq!(top.children().len(), 2, "{name}: header and notice slot");
            heights.push((top.children()[1].bounds().height, body.bounds().y));
        }
        assert!(
            heights.windows(2).all(|w| (w[0].1 - w[1].1).abs() < 0.5),
            "{heights:?}"
        );
        assert!(heights.iter().all(|(h, _)| *h == 0.0), "{heights:?}");
        // With a failure the line has room and the body moves down by it.
        let app = page_app("settings-notifications-long");
        let laid = Laid::new(&app, &renderer, WIDE);
        let body = find_body(&laid.node).expect("the body");
        assert!(body.bounds().y > heights[0].1 + 20.0);
    }
}

mod sessions_screen {
    use iced::advanced::widget::{Id, Operation, operation::Focusable};
    use twicecat_app::screens::sessions::{CARD_WIDTH, WRONG_PASSWORD, trust_severity};
    use twicecat_app::sessions::{REAUTH_INPUT_ID, RENAME_INPUT_ID};
    use twicecat_core::api::DeviceTrust;

    use super::*;

    const SCREENS: [&str; 13] = [
        "sessions",
        "sessions-loading",
        "sessions-confirm-one",
        "sessions-confirm-all",
        "sessions-rename",
        "sessions-long-names",
        "sessions-recovery-off",
        "sessions-recovery-on",
        "sessions-notice",
        "sessions-approval",
        "sessions-reauth",
        "sessions-reauth-failed",
        "sessions-logout-warning",
    ];

    /// The first node `width` wide that is a card (not a full-window layer).
    fn find_card(node: &Node, width: f32) -> Option<&Node> {
        if (node.bounds().width - width).abs() < 0.5 && node.bounds().height < 700.0 {
            return Some(node);
        }
        node.children().iter().find_map(|c| find_card(c, width))
    }

    fn app_of(name: &str) -> App {
        let mut app = scenario_app(name);
        app.set_ui_settings(variants(false)[0].settings());
        app.window_width = WIDE.width;
        app
    }

    fn card_width_of(name: &str) -> f32 {
        match name {
            "sessions-reauth" | "sessions-reauth-failed" | "sessions-logout-warning" => 520.0,
            _ => CARD_WIDTH,
        }
    }

    #[test]
    fn the_sessions_card_is_760_by_620_and_the_prompts_are_520_wide_and_centred() {
        let renderer = renderer();
        for name in SCREENS.into_iter().filter(|n| !n.starts_with("settings")) {
            let app = app_of(name);
            let laid = Laid::new(&app, &renderer, WIDE);
            if name.starts_with("sessions-reauth") || name == "sessions-logout-warning" {
                // The prompt is the last layer: a card of its own width.
                let card = find_card(&laid.node, 520.0)
                    .unwrap_or_else(|| panic!("{name}: no 520 px card"));
                let mid = card.bounds().x + card.bounds().width / 2.0;
                assert!((mid - WIDE.width / 2.0).abs() < 1.0, "{name}: not centred");
            } else {
                let card = find_card(&laid.node, 760.0)
                    .unwrap_or_else(|| panic!("{name}: no 760 px card"));
                assert_eq!(card.bounds().height, 620.0, "{name}");
            }
        }
    }

    #[test]
    fn nothing_runs_out_of_a_card_sideways_at_both_widths_and_densities() {
        let renderer = renderer();
        for name in SCREENS {
            for size in SIZES {
                for density in DENSITIES {
                    let mut app = app_of(name);
                    with_settings(&mut app, density, RoomListChoice::Expanded);
                    app.window_width = size.width;
                    let laid = Laid::new(&app, &renderer, size);
                    let width = card_width_of(name);
                    let Some(card) = find_card(&laid.node, width.min(size.width)) else {
                        panic!("{name} at {}: no card", size.width);
                    };
                    let mut out = Vec::new();
                    hspills(card, iced::Point::ORIGIN, &mut out);
                    assert!(
                        out.is_empty(),
                        "{name} {} at {}: {out:?}",
                        density.1,
                        size.width
                    );
                }
            }
        }
    }

    #[test]
    fn a_small_window_shrinks_the_card_to_fit() {
        let renderer = renderer();
        let size = Size::new(1000.0, 420.0);
        let mut app = app_of("sessions");
        app.window_width = size.width;
        let laid = Laid::new(&app, &renderer, size);
        let card = find_card(&laid.node, 760.0).expect("the card");
        assert!(card.bounds().height <= size.height, "{:?}", card.bounds());
        assert!(card.bounds().height >= 280.0, "{:?}", card.bounds());
    }

    fn shown(name: &str) -> Vec<(String, Rectangle)> {
        let renderer = renderer();
        let app = app_of(name);
        let mut laid = Laid::new(&app, &renderer, WIDE);
        texts(&mut laid, &renderer)
    }

    #[test]
    fn this_session_is_first_and_each_shows_its_trust_in_words() {
        let list = shown("sessions");
        let y = |wanted: &str| {
            list.iter()
                .find(|(t, _)| t == wanted)
                .unwrap_or_else(|| panic!("no text {wanted:?}"))
                .1
                .y
        };
        assert!(y("Twicecat on this computer") < y("Phone"));
        assert!(
            y("Phone") < y("OLDLAPTOP"),
            "an unnamed session shows its ID"
        );
        for words in [
            "This device",
            "Verified",
            "Not verified",
            "No encryption keys",
        ] {
            assert!(list.iter().any(|(t, _)| t == words), "no tag {words:?}");
        }
        // The tag of this device is on the first row, with its trust.
        assert!((y("This device") - y("Verified")).abs() < 1.0);
        // The colour says it too: trusted is not drawn as a warning.
        assert_eq!(
            trust_severity(DeviceTrust::Verified),
            twicecat_app::ui::kit::Severity::Success
        );
        assert_eq!(
            trust_severity(DeviceTrust::Unverified),
            twicecat_app::ui::kit::Severity::Warning
        );
        assert_eq!(
            trust_severity(DeviceTrust::NoKeys),
            twicecat_app::ui::kit::Severity::Warning
        );
    }

    #[test]
    fn a_long_name_wraps_and_the_end_of_a_long_id_is_never_clipped() {
        let list = shown("sessions-long-names");
        // The name is capped (64 characters) and one line; it wraps inside
        // its row instead of running off it.
        let name = list
            .iter()
            .find(|(t, _)| t.starts_with("NNNN"))
            .expect("the long name");
        assert!(name.0.chars().count() <= 64, "{}", name.0.chars().count());
        assert!(name.1.height > 30.0, "the name did not wrap: {:?}", name.1);
        // The ID is cut in the middle; its last characters stay.
        let id = list
            .iter()
            .find(|(t, _)| t.contains("ENDMARK"))
            .expect("the end of the long ID is drawn");
        assert!(id.0.ends_with("ENDMARK"), "{}", id.0);
        assert!(id.0.chars().count() <= 64);
        // Controls of the server's text: no direction override or line break.
        for (t, _) in &list {
            assert!(!t.contains('\u{202E}') && !t.contains('\n'), "{t:?}");
        }
        // A name of blanks falls back to the ID.
        assert!(list.iter().any(|(t, _)| t == "BLANK1"));
    }

    /// The messages (as text) of every control, with where it sits. A control
    /// in the scrolling list is cut to the list's window: what the scroll
    /// clips cannot be clicked, so it does not count as near the footer.
    fn controls(name: &str) -> Vec<(Rectangle, String)> {
        let renderer = renderer();
        let app = app_of(name);
        let list = senders(&app, &renderer, WIDE);
        let laid = Laid::new(&app, &renderer, WIDE);
        let clip = find_card(&laid.node, 760.0).and_then(|card| {
            // card > content > column [header, bars, list, footer]
            let column = card.children().first()?.children().first()?;
            let scroll = column.children().get(2)?;
            let origin = origin_of(&laid.node, scroll, iced::Point::ORIGIN)?;
            Some(Rectangle::new(origin, scroll.bounds().size()))
        });
        list.into_iter()
            .map(|(rect, text)| match clip {
                Some(clip) if clip.contains(rect.center()) => {
                    (rect.intersection(&clip).unwrap_or(rect), text)
                }
                _ => (rect, text),
            })
            .collect()
    }

    fn sends(list: &[(Rectangle, String)], prefix: &str) -> Vec<Rectangle> {
        list.iter()
            .filter(|(_, t)| t.starts_with(prefix))
            .map(|(r, _)| *r)
            .collect()
    }

    const DANGEROUS: [&str; 4] = [
        "Sessions(SignOutAsk(",
        "Sessions(SignOutAllAsk)",
        "Sessions(SignOutConfirmed)",
        "Sessions(LogoutAnyway)",
    ];

    #[test]
    fn no_confirmation_can_be_reached_before_it_is_asked() {
        // Nothing on the plain screen, the warning-less states or the
        // prompts confirms a sign-out or logs out.
        for name in [
            "sessions",
            "sessions-long-names",
            "sessions-rename",
            "sessions-recovery-off",
        ] {
            for (_, t) in controls(name) {
                for confirm in ["SignOutConfirmed", "LogoutAnyway", "ReauthSubmitted"] {
                    assert!(!t.contains(confirm), "{name}: {t}");
                }
            }
        }
        // The password's Continue is off while the field is empty.
        for name in ["sessions-reauth", "sessions-reauth-failed"] {
            assert!(
                !controls(name)
                    .iter()
                    .any(|(_, t)| t.contains("ReauthSubmitted")),
                "{name}: Continue is on with no password"
            );
        }
    }

    #[test]
    fn a_second_click_where_the_sign_out_asked_does_not_confirm_it() {
        let renderer = renderer();
        for (plain, asked, prefix) in [
            ("sessions", "sessions-confirm-one", "Sessions(SignOutAsk("),
            (
                "sessions",
                "sessions-confirm-all",
                "Sessions(SignOutAllAsk)",
            ),
        ] {
            let before = controls(plain);
            let asks = sends(&before, prefix);
            assert!(!asks.is_empty(), "{plain}: no {prefix}");
            let app = app_of(asked);
            for ask in asks {
                let mut laid = Laid::new(&app, &renderer, WIDE);
                laid.settle(&renderer, WIDE);
                for at in [
                    ask.center(),
                    iced::Point::new(ask.x + 4.0, ask.center().y),
                    iced::Point::new(ask.x + ask.width - 4.0, ask.center().y),
                ] {
                    let sent: Vec<String> = click(&mut laid, &renderer, WIDE, at)
                        .iter()
                        .map(|m| format!("{m:?}"))
                        .collect();
                    assert!(
                        !sent.iter().any(|m| m.contains("SignOutConfirmed")),
                        "{asked}: a click at {at:?} confirms: {sent:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn destructive_buttons_sit_at_least_one_gap_from_every_other_control() {
        for name in [
            "sessions",
            "sessions-confirm-one",
            "sessions-confirm-all",
            "sessions-long-names",
            "sessions-logout-warning",
            "sessions-recovery-off",
        ] {
            let list = controls(name);
            let danger: Vec<&(Rectangle, String)> = list
                .iter()
                .filter(|(_, t)| DANGEROUS.iter().any(|d| t.starts_with(d)))
                .collect();
            assert!(!danger.is_empty(), "{name}: no destructive button");
            for (rect, text) in &danger {
                for (other, other_text) in &list {
                    if other_text == text {
                        continue;
                    }
                    let distance = gap(*rect, *other);
                    assert!(
                        distance >= 12.0 - 0.5,
                        "{name}: {text} is {distance} px from {other_text}"
                    );
                }
            }
        }
    }

    #[test]
    fn verify_is_offered_only_for_another_unverified_session_and_sign_out_never_for_this_one() {
        let list = controls("sessions");
        let all: Vec<&String> = list.iter().map(|(_, t)| t).collect();
        assert_eq!(all.iter().filter(|t| t.contains("Verify(")).count(), 1);
        assert!(
            all.iter()
                .any(|t| t.contains(r#"Verify(DeviceId("PHONE1"))"#))
        );
        assert!(
            !all.iter()
                .any(|t| t.contains("SignOutAsk") && t.contains("GALLERYDEV"))
        );
        assert_eq!(all.iter().filter(|t| t.contains("SignOutAsk(")).count(), 2);
        assert_eq!(all.iter().filter(|t| t.contains("RenameStart(")).count(), 3);
        assert!(all.iter().any(|t| t.contains("SignOutAllAsk")));
    }

    #[test]
    fn a_prompt_over_the_page_takes_every_click() {
        for name in ["sessions-reauth", "sessions-reauth-failed"] {
            for (_, t) in controls(name) {
                assert!(
                    t.contains("ReauthCancelled"),
                    "{name}: a click got through to the page: {t}"
                );
            }
        }
        let warning = controls("sessions-logout-warning");
        let kinds: Vec<&str> = warning.iter().map(|(_, t)| t.as_str()).collect();
        for wanted in ["LogoutSetupRecovery", "LogoutAnyway", "LogoutCancelled"] {
            assert!(kinds.iter().any(|t| t.contains(wanted)), "no {wanted}");
        }
        // The way out first, the sign-out last.
        let y = |wanted: &str| {
            warning
                .iter()
                .find(|(_, t)| t.contains(wanted))
                .expect("the button")
                .0
                .y
        };
        assert!(y("LogoutSetupRecovery") < y("LogoutAnyway"));
    }

    #[test]
    fn the_wrong_password_line_and_the_warning_words_are_there() {
        let plain = shown("sessions-reauth");
        let failed = shown("sessions-reauth-failed");
        assert!(!plain.iter().any(|(t, _)| t == WRONG_PASSWORD));
        assert!(failed.iter().any(|(t, _)| t == WRONG_PASSWORD));
        assert!(
            plain
                .iter()
                .any(|(t, _)| t.starts_with("Your homeserver needs your account password"))
        );
        let warning = shown("sessions-logout-warning");
        assert!(
            warning
                .iter()
                .any(|(t, _)| t.starts_with("You'll lose access"))
        );
    }

    /// The words of a bar and its buttons: the buttons are on a row of their
    /// own below the words, so nothing covers the text.
    #[test]
    fn bars_keep_their_buttons_below_their_words() {
        for (name, words, button) in [
            (
                "sessions-approval",
                "account.example.org",
                "ApprovalConfirmed",
            ),
            (
                "sessions-notice",
                "The session could not be renamed.",
                "DismissNotice",
            ),
            ("sessions-recovery-off", "Recovery is off.", "SetupRecovery"),
            (
                "sessions-recovery-on",
                "Recovery is on.",
                "ChangeKeyRequested",
            ),
        ] {
            let texts = shown(name);
            let at = texts
                .iter()
                .find(|(t, _)| t.contains(words))
                .unwrap_or_else(|| panic!("{name}: no {words:?}"))
                .1;
            let buttons = controls(name);
            let button = buttons
                .iter()
                .find(|(_, t)| t.contains(button))
                .unwrap_or_else(|| panic!("{name}: no {button}"))
                .0;
            assert!(
                button.y >= at.y + at.height - 0.5,
                "{name}: {button:?} is not below the words {at:?}"
            );
        }
    }

    #[test]
    fn the_footer_keeps_its_button_in_the_card_when_the_list_is_long() {
        let renderer = renderer();
        let app = app_of("sessions-long-names");
        let laid = Laid::new(&app, &renderer, WIDE);
        let card = find_card(&laid.node, 760.0).expect("the card").bounds();
        let list = controls("sessions-long-names");
        let all = sends(&list, "Sessions(SignOutAllAsk)");
        assert_eq!(all.len(), 1);
        assert!(all[0].y + all[0].height <= card.y + 620.0, "{all:?}");
    }

    struct Stops(Vec<String>);

    impl Operation for Stops {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn Focusable) {
            self.0.push(format!("{id:?}"));
        }
    }

    #[test]
    fn the_rename_and_password_fields_are_the_text_stops() {
        let renderer = renderer();
        let id_of = |name: &'static str| format!("{:?}", Some(Id::new(name)));
        for (name, wanted) in [
            ("sessions", None),
            ("sessions-rename", Some(id_of(RENAME_INPUT_ID))),
            ("sessions-reauth", Some(id_of(REAUTH_INPUT_ID))),
        ] {
            let app = app_of(name);
            let mut laid = Laid::new(&app, &renderer, WIDE);
            let mut op = Stops(Vec::new());
            laid.element.as_widget_mut().operate(
                &mut laid.tree,
                Layout::new(&laid.node),
                &renderer,
                &mut op,
            );
            let mine: Vec<String> =
                op.0.into_iter()
                    .filter(|id| id.contains("sessions-"))
                    .collect();
            assert_eq!(mine, wanted.into_iter().collect::<Vec<_>>(), "{name}");
        }
    }
}
