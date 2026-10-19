//! The app's icon for the window: the 256-pixel PNG that
//! `scripts/make-icons.sh` renders from `packaging/icons/`. (The tray draws
//! the same shape in code, `tray.rs`.)

/// The 256-pixel icon.
pub const WINDOW_ICON_PNG: &[u8] = include_bytes!(
    "../../../../packaging/icons/hicolor/256x256/apps/io.github.ga_lep.Twicecat.png"
);

/// The window icon, or `None` if the PNG does not decode.
pub fn window_icon() -> Option<iced::window::Icon> {
    let image = image::load_from_memory_with_format(WINDOW_ICON_PNG, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    iced::window::icon::from_rgba(image.into_raw(), width, height).ok()
}
