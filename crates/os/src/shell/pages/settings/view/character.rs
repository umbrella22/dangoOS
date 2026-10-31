//! Character settings page.
use super::components::{checkmark, entry, list, loading_spinner, note};
use crate::shell::{
    components::page::{font_20, text},
    messages::AppMsg,
    pages::settings::model::Session,
    theme::tokens::*,
};
use rs_lvgl_ui::{Align, FlexAlign, Key, Size, Style, View};

pub(super) fn view(session: &Session) -> View<AppMsg> {
    let info = session.info;
    let mut body = list("character-choices");
    for (index, choice) in session.skins.iter().enumerate() {
        let selected = choice.id == session.selected_skin;
        let loading = session.loading_skin.as_deref() == Some(choice.id.as_str());
        let mut choice_view = entry(
            Key::Id(index as u64),
            choice.name.clone(),
            "",
            AppMsg::SelectSkin(choice.id.clone()),
        )
        .style(
            Style::default()
                .height(Size::Px(56))
                .border(if selected { ACCENT } else { CARD }, 2)
                .bg(if selected { CARD_HI } else { CARD }),
        )
        .clickable(!info.busy && session.loading_skin.is_none() && info.storage_ready && !selected);
        if loading {
            choice_view = choice_view.child(
                rs_lvgl_ui::row([])
                    .key(Key::Named("skin-loading"))
                    .scrollable(false)
                    .clickable(false)
                    .style(
                        Style::neutral()
                            .width(Size::Content)
                            .height(Size::Px(24))
                            .gap(8)
                            .flex_align(FlexAlign::Center, FlexAlign::Center)
                            .align(Align::RightMid),
                    )
                    .child(loading_spinner("skin-spinner"))
                    .child(text("切换中", font_20(), TEXT)),
            );
        } else if selected {
            choice_view = choice_view.child(
                checkmark("skin-selected", ACCENT).style(Style::default().align(Align::RightMid)),
            );
        }
        body = body.child(choice_view);
    }
    if !info.storage_ready {
        body = body.child(note("character-loading", "正在读取团子形象"));
    } else if session.skins.len() <= 1 {
        body = body.child(note("character-empty", "安装团子形象后可在这里切换"));
    }
    if let Some(message) = session.message {
        body = body.child(note("character-message", message));
    }
    body
}
