//! Table widget -- scrollable data grid with sortable columns.
//!
//! Renders a header row (optional) and data rows from `columns` and
//! `rows` JSON props. Columns define key, label, alignment, width,
//! and sortability. Clicking a sortable column header emits a `sort`
//! event with the column key. Separator styling and text sizes are
//! configurable.

use iced::widget::{button, column, container, row, rule, scrollable, text};
use iced::{Element, Fill, Length, alignment};
use serde_json::Value;

use super::helpers::*;
use crate::extensions::RenderCtx;
use crate::message::Message;
use crate::protocol::TreeNode;

/// Parsed column descriptor from the "columns" prop.
struct TableColumn {
    key: String,
    label: String,
    align: alignment::Horizontal,
    width: Length,
    sortable: bool,
}

fn parse_table_columns(props: Props<'_>) -> Vec<TableColumn> {
    props
        .and_then(|p| p.get("columns"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|col| {
                    let key = col.get("key")?.as_str()?.to_owned();
                    let label = col
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&key)
                        .to_owned();
                    let align = col
                        .get("align")
                        .and_then(|v| v.as_str())
                        .and_then(value_to_horizontal_alignment)
                        .unwrap_or(alignment::Horizontal::Left);
                    let width = col
                        .get("width")
                        .and_then(value_to_length)
                        .unwrap_or(Length::FillPortion(1));
                    let sortable = col
                        .get("sortable")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    Some(TableColumn {
                        key,
                        label,
                        align,
                        width,
                        sortable,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn render_table<'a>(node: &'a TreeNode, _ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Fill);
    let show_header = prop_bool_default(props, "header", true);
    let padding_val = parse_padding_value(props);
    let table_id = node.id.clone();

    let header_text_size = prop_f32(props, "header_text_size").unwrap_or(14.0);
    let row_text_size = prop_f32(props, "row_text_size").unwrap_or(13.0);

    let cell_spacing = prop_f32(props, "cell_spacing").unwrap_or(4.0);
    let row_spacing = prop_f32(props, "row_spacing").unwrap_or(2.0);
    let separator_thickness = prop_f32(props, "separator_thickness").unwrap_or(1.0);
    let separator_color = prop_color(props, "separator_color");

    let sort_by = prop_str(props, "sort_by");
    let sort_order = prop_str(props, "sort_order");

    let columns = parse_table_columns(props);

    // "rows" is an array of objects.
    let rows: Vec<&Value> = props
        .and_then(|p| p.get("rows"))
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().collect())
        .unwrap_or_default();

    if columns.is_empty() {
        return text("(empty table)").into();
    }

    let mut table_rows: Vec<Element<'a, Message>> = Vec::new();

    // Header row (conditional)
    if show_header {
        let header_cells: Vec<Element<'a, Message>> = columns
            .iter()
            .map(|col| {
                // Build sort indicator if this column is currently sorted.
                let sort_indicator = if sort_by.as_deref() == Some(&col.key) {
                    match sort_order.as_deref() {
                        Some("asc") => " \u{25B2}",
                        Some("desc") => " \u{25BC}",
                        _ => "",
                    }
                } else {
                    ""
                };

                let label_text = format!("{}{}", col.label, sort_indicator);

                if col.sortable {
                    let click_id = table_id.clone();
                    let click_key = col.key.clone();
                    container(
                        button(text(label_text).size(header_text_size))
                            .on_press(Message::Event {
                                id: click_id,
                                data: serde_json::json!({"column": click_key}),
                                family: "sort".into(),
                            })
                            .style(button::text),
                    )
                    .width(col.width)
                    .align_x(col.align)
                    .into()
                } else {
                    container(text(label_text).size(header_text_size))
                        .width(col.width)
                        .align_x(col.align)
                        .into()
                }
            })
            .collect();
        let header = row(header_cells).spacing(cell_spacing).width(Fill);
        table_rows.push(header.into());

        // Separator
        let show_separator = prop_bool_default(props, "separator", true);
        if show_separator {
            let sep: Element<'a, Message> = if let Some(sep_col) = separator_color {
                rule::horizontal(separator_thickness)
                    .style(move |_theme: &iced::Theme| rule::Style {
                        color: sep_col,
                        radius: Default::default(),
                        fill_mode: rule::FillMode::Full,
                        snap: true,
                    })
                    .into()
            } else {
                rule::horizontal(separator_thickness).into()
            };
            table_rows.push(sep);
        }
    }

    // Data rows
    for data_row in &rows {
        let cells: Vec<Element<'a, Message>> = columns
            .iter()
            .map(|col| {
                let cell_text = data_row
                    .get(&col.key)
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                container(text(cell_text).size(row_text_size))
                    .width(col.width)
                    .align_x(col.align)
                    .into()
            })
            .collect();
        table_rows.push(row(cells).spacing(cell_spacing).width(Fill).into());
    }

    scrollable(
        column(table_rows)
            .spacing(row_spacing)
            .width(width)
            .padding(padding_val),
    )
    .into()
}
