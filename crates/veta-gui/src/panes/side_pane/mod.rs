//! The side pane: tabs for applied steps, statistics and file details.

use super::*;
use iced::widget::column;

mod file;
mod stats;
mod steps;

use file::*;
pub use stats::*;
use steps::*;

const SIDE_PANE_WIDTH: f32 = 300.0;

/// The side pane: applied steps or file details.
pub fn side_pane<'a>(
    tab: &'a Tab,
    doc: &'a Document,
    side: SideTab,
    stats: StatsView,
    ui: Ui,
) -> Element<'a, Message> {
    let t = ui.tokens;
    let tab_button = |label: &str, target: SideTab| {
        let selected = side == target;
        button(text(tr(label)).size(ui.small()))
            .padding([4, 10])
            .style(move |_theme: &Theme, status| {
                let mut style = subtle_button(t, status);
                if selected {
                    style.text_color = t.primary;
                    style.border = Border {
                        color: t.primary,
                        width: 0.0,
                        radius: 4.0.into(),
                    };
                    style.background = Some(Background::Color(Color {
                        a: 0.1,
                        ..t.primary
                    }));
                }
                style
            })
            .on_press(Message::SideTab(target))
    };
    let tabs = row![
        tab_button("ST_SIDE_TAB_STEPS", SideTab::Steps),
        tab_button("ST_SIDE_TAB_STATS", SideTab::Stats),
        tab_button("ST_SIDE_TAB_FILE", SideTab::File)
    ]
    .spacing(4);
    let body = match side {
        SideTab::Steps => steps_pane(tab, doc, ui),
        SideTab::Stats => stats_pane(stats, ui),
        SideTab::File => file_details(doc, ui),
    };
    container(column![container(tabs).padding([8, 12]), body])
        .width(SIDE_PANE_WIDTH)
        .height(Length::Fill)
        .style(move |_theme: &Theme| container::Style::default().background(t.background))
        .into()
}
