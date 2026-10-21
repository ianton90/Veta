//! The File tab: file details and metadata.

use super::*;
use iced::widget::column;

/// Read-only file details and metadata.
pub(super) fn file_details(doc: &Document, ui: Ui) -> Element<'_, Message> {
    let t = ui.tokens;
    // Labels are tags.
    let mut fields: Vec<(&str, String)> = vec![
        ("ST_FILE_ROWS", doc.num_rows().to_string()),
        ("ST_FILE_COLUMNS", doc.num_columns().to_string()),
        (
            "ST_FILE_LOADED",
            tr(match doc.source_mode() {
                SourceMode::InMemory => "ST_STATUS_IN_MEMORY",
                SourceMode::Paged => "ST_FILE_PAGED",
            }),
        ),
    ];
    if let Some(info) = doc.file_info() {
        fields.push(("ST_FILE_SIZE", human_bytes(info.file_size)));
        fields.push((
            "ST_FILE_UNCOMPRESSED",
            human_bytes(info.uncompressed_size()),
        ));
        fields.push(("ST_FILE_ROW_GROUPS", info.row_groups.len().to_string()));
    }
    fields.push((
        "ST_FILE_FORMAT",
        doc.writer_settings().format_version.to_string(),
    ));
    if let Some(created_by) = &doc.metadata().created_by {
        fields.push(("ST_FILE_CREATED_BY", created_by.clone()));
    }
    let mut items =
        column![text(tr("ST_SIDE_TAB_FILE")).size(ui.heading()).font(bold())].spacing(6);
    for (label, value) in fields {
        items = items.push(
            row![
                text(tr(label))
                    .size(ui.small())
                    .width(110)
                    .color(t.muted_text),
                text(value).size(ui.small()),
            ]
            .spacing(8),
        );
    }

    // The Arrow schema entry is internal to the file format.
    let metadata: Vec<_> = doc
        .metadata()
        .key_value
        .iter()
        .filter(|kv| kv.key != veta_core::io::ARROW_SCHEMA_KEY)
        .collect();
    let mut meta = column![
        row![
            text(tr("ST_DLG_METADATA_TITLE"))
                .size(ui.heading())
                .font(bold()),
            iced::widget::Space::new().width(Length::Fill),
            button(text(tr("ST_FILE_EDIT")).size(ui.small()))
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(Message::Action(crate::ribbon::Action::FileMetadata)),
        ]
        .align_y(Alignment::Center)
    ]
    .spacing(6);
    if metadata.is_empty() {
        meta = meta.push(
            text(tr("ST_FILE_NONE"))
                .size(ui.small())
                .color(t.muted_text),
        );
    }
    for kv in metadata {
        meta = meta.push(
            column![
                text(kv.key.clone()).size(ui.small()).font(bold()),
                text(abbreviate(kv.value.as_deref().unwrap_or_default(), 40)).size(ui.small()),
            ]
            .spacing(2),
        );
    }

    scrollable(column![items, meta].spacing(20).padding(12))
        .height(Length::Fill)
        .into()
}
