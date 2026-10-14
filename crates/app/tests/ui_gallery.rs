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
        "ring-dm",
        "ring-group",
        "voice-audio",
        "settings",
        "sessions",
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
    let laid = Laid::new(&app, &renderer, WIDE);
    assert_eq!(room_pane_place(&laid), (place, shape), "everything open");
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
    let mut shots: Vec<(&str, Vec<(String, String)>)> = Vec::new();
    for scenario in &scenarios {
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
            (RoomListChoice::Expanded, 720.0),
            (RoomListChoice::Expanded, 600.0),
            (RoomListChoice::Rail, 480.0),
            (RoomListChoice::Rail, 520.0),
        ] {
            let mut settings = variants(false)[0].settings();
            settings.room_list = list;
            app.set_ui_settings(settings);
            let size = Size::new(width, 800.0);
            let laid = Laid::new(&app, &renderer, size);
            let header = header_node(&laid);
            let mut out = Vec::new();
            spills(header, iced::Point::ORIGIN, &mut out);
            assert!(
                out.is_empty(),
                "{trust:?} at {width}: header content spills out: {out:?}"
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
                    let laid = Laid::new(&app, &renderer, size);
                    let pane = list_pane_node(&laid);
                    let what = format!("{list:?} {} call={in_call} {size:?}", density.1);
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
