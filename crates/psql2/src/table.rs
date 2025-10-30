use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets, ContentArrangement, Table};

use supports_unicode::Stream;

pub fn configure(table: &mut Table) {
	if supports_unicode::on(Stream::Stdout) {
		table.load_preset(presets::UTF8_FULL);
		table.apply_modifier(UTF8_ROUND_CORNERS);
	} else {
		table.load_preset(presets::ASCII_FULL);
	}

	table.set_content_arrangement(ContentArrangement::Dynamic);

	if let Ok((width, _)) = crossterm::terminal::size() {
		table.set_width(width);
	}
}
