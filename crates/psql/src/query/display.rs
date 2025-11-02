use miette::Result;
use tokio::io::AsyncWrite;

mod csv;
mod expanded;
mod json;
mod normal;

/// Context for displaying query results.
pub struct DisplayContext<'a, W: AsyncWrite + Unpin> {
	pub columns: &'a [tokio_postgres::Column],
	pub rows: &'a [tokio_postgres::Row],
	pub unprintable_columns: &'a [usize],
	pub text_rows: &'a Option<Vec<tokio_postgres::Row>>,
	pub writer: &'a mut W,
	pub use_colours: bool,
	pub theme: crate::theme::Theme,
	/// Optional column indices to filter display (None means show all columns)
	pub column_indices: Option<&'a [usize]>,
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
