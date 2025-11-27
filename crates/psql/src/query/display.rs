use std::sync::Arc;

use bestool_postgres::text_cast::TextCaster;
use miette::Result;
use tokio::io::AsyncWrite;

use crate::{Config, column_extractor::ColumnRef};

mod csv;
mod excel;
mod expanded;
mod json;
mod normal;
mod sqlite;

/// Context for displaying query results.
pub struct DisplayContext<'a, W: AsyncWrite + Unpin> {
	pub config: &'a Arc<Config>,
	pub columns: &'a [tokio_postgres::Column],
	pub rows: &'a [tokio_postgres::Row],
	pub unprintable_columns: &'a [usize],
	pub text_caster: Option<TextCaster>,
	pub writer: &'a mut W,
	pub use_colours: bool,
	/// Optional column indices to filter display (None means show all columns)
	pub column_indices: Option<&'a [usize]>,
	/// Whether redaction mode is enabled
	pub redact_mode: bool,
	/// Extracted column references from the query (for redaction matching)
	pub column_refs: &'a [ColumnRef],
}

pub async fn display<W: AsyncWrite + Unpin>(
	ctx: &mut DisplayContext<'_, W>,
	is_json: bool,
	is_expanded: bool,
) -> Result<()> {
	if is_json {
		json::display(ctx, is_expanded).await
	} else if is_expanded {
		expanded::display(ctx).await
	} else {
		normal::display(ctx).await
	}
}

pub async fn display_csv<W: AsyncWrite + Unpin>(ctx: &mut DisplayContext<'_, W>) -> Result<()> {
	csv::display(ctx).await
}

pub async fn display_excel<W: AsyncWrite + Unpin>(
	ctx: &mut DisplayContext<'_, W>,
	file_path: &str,
) -> Result<()> {
	excel::display(ctx, file_path).await
}

pub async fn display_sqlite<W: AsyncWrite + Unpin>(
	ctx: &mut DisplayContext<'_, W>,
	file_path: &str,
) -> Result<()> {
	sqlite::display(ctx, file_path).await
}
