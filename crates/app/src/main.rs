//! Tessera desktop app: opens one empty window.

use iced::Element;
use iced::widget::space;

const TITLE: &str = "Tessera";

fn update(_state: &mut (), _message: ()) {}

fn view(_state: &()) -> Element<'_, ()> {
    space().into()
}

fn main() -> iced::Result {
    iced::application(<()>::default, update, view)
        .title(TITLE)
        .run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_title_is_tessera() {
        assert_eq!(super::TITLE, "Tessera");
    }
}
