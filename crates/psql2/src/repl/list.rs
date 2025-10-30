use std::ops::ControlFlow;

use crate::parser::ListItem;

use super::state::ReplContext;

mod index;
mod pattern;
mod table;

pub async fn handle_list(
	ctx: &mut ReplContext<'_>,
	item: ListItem,
	pattern: String,
	detail: bool,
	sameconn: bool,
) -> ControlFlow<()> {
	match item {
		ListItem::Table => table::handle_list_tables(ctx, &pattern, detail, sameconn).await,
		ListItem::Index => index::handle_list_indexes(ctx, &pattern, detail, sameconn).await,
	}
}
