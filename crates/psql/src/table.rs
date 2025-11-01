use comfy_table::{CellAlignment, ContentArrangement, Table, presets};

use supports_unicode::Stream;

pub fn configure(table: &mut Table) {
	if supports_unicode::on(Stream::Stdout) {
		table.load_preset(presets::UTF8_NO_BORDERS);
	} else {
		table.load_preset(presets::ASCII_NO_BORDERS);
	}

	table.set_content_arrangement(ContentArrangement::Dynamic);

	if let Ok((width, _)) = crossterm::terminal::size() {
		table.set_width(width);
	}
}

pub fn style_header(table: &mut Table) {
	if let Some(header) = table.header() {
		let mut new = Vec::with_capacity(header.cell_count());
		for cell in header.cell_iter() {
			new.push(
				cell.clone()
					.add_attribute(comfy_table::Attribute::Bold)
					.set_alignment(CellAlignment::Center),
			);
		}

		table.set_header(new);
	}
}
