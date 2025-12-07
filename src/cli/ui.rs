use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Attribute, Cell, CellAlignment, Color, ContentArrangement, Table};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

/// Defines different styles for text elements.
pub enum StyleType {
    Title,
    TotalLabel,
    TotalValue,
    Error,
    Subtle,
}

/// Applies a consistent style to a string.
pub fn style_text(text: &str, style_type: StyleType) -> String {
    let styled = match style_type {
        StyleType::Title => style(text).bold().underlined(),
        StyleType::TotalLabel => style(text).bold(),
        StyleType::TotalValue => style(text).green().bold(),
        StyleType::Error => style(text).red(),
        StyleType::Subtle => style(text).dim(),
    };
    styled.to_string()
}

/// Creates a new `comfy_table::Table` with standard styling.
pub fn new_styled_table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table
}

/// Creates a styled header cell for a table.
pub fn header_cell(text: &str) -> Cell {
    Cell::new(text)
        .fg(Color::Cyan)
        .add_attribute(Attribute::Bold)
}

/// Formats an `Option<T>` into a `Cell`. `None` is displayed as "N/A".
pub fn format_optional_cell<T>(value: Option<T>, format_fn: impl Fn(T) -> String) -> Cell {
    value.map_or(
        Cell::new("N/A")
            .fg(Color::DarkGrey)
            .set_alignment(CellAlignment::Right),
        |v| Cell::new(format_fn(v)).set_alignment(CellAlignment::Right),
    )
}

/// Formats a cell with bold and green text
pub fn format_percentage_cell(value: f64, format_fn: impl Fn(f64) -> String) -> Cell {
    Cell::new(format_fn(value))
        .add_attribute(Attribute::Bold)
        .fg(Color::Green)
        .set_alignment(CellAlignment::Right)
}

/// Creates a cell for displaying percentage change with color coding.
pub fn change_cell(change: f64) -> Cell {
    let text = format!("{change:.2}%");
    if change >= 0.0 {
        Cell::new(text)
            .fg(Color::Green)
            .set_alignment(CellAlignment::Right)
    } else {
        Cell::new(text)
            .fg(Color::Red)
            .set_alignment(CellAlignment::Right)
    }
}

/// Creates a cell for "N/A" values, with error-specific styling.
pub fn na_cell(has_error: bool) -> Cell {
    let color = if has_error {
        Color::Red
    } else {
        Color::DarkGrey
    };
    Cell::new("N/A").fg(color)
}

/// Creates a new `indicatif::ProgressBar` with standard styling.
pub fn new_progress_bar(len: u64, with_message: bool) -> ProgressBar {
    let template = if with_message {
        "{spinner:.green} {msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})"
    } else {
        "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})"
    };

    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(template)
            .unwrap()
            .progress_chars("#>-"),
    );
    pb
}

/// Prints a separator line matching the terminal width.
pub fn print_separator() {
    let term_width = console::Term::stdout()
        .size_checked()
        .map(|(_, w)| w as usize)
        .unwrap_or(80);
    println!("\n{}", "─".repeat(term_width));
}
