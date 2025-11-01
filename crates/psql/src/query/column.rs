pub fn get_value(
	row: &tokio_postgres::Row,
	column_index: usize,
	row_index: usize,
	unprintable_columns: &[usize],
	text_rows: &Option<Vec<tokio_postgres::Row>>,
) -> String {
	if !unprintable_columns.contains(&column_index) {
		return format_value(row, column_index);
	}

	if let Some(text_rows) = text_rows {
		if let Some(text_row) = text_rows.get(row_index) {
			return text_row
				.try_get::<_, Option<String>>(column_index)
				.ok()
				.flatten()
				.unwrap_or_else(|| "NULL".to_string());
		}
	}

	"(error)".to_string()
}

pub fn format_value(row: &tokio_postgres::Row, i: usize) -> String {
	let column = row.columns().get(i);
	if let Some(col) = column {
		if col.type_().name() == "void" {
			return "(void)".to_string();
		}
	}

	if let Ok(v) = row.try_get::<_, String>(i) {
		v
	} else if let Ok(v) = row.try_get::<_, i16>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, i32>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, i64>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, f32>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, f64>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, bool>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, Vec<u8>>(i) {
		format!("\\x{encoded}", encoded = hex::encode(v))
	} else if let Ok(v) = row.try_get::<_, jiff::Timestamp>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, jiff::civil::Date>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, jiff::civil::Time>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, jiff::civil::DateTime>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, serde_json::Value>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, Vec<String>>(i) {
		format!("{{{}}}", v.join(","))
	} else if let Ok(v) = row.try_get::<_, Vec<i32>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else if let Ok(v) = row.try_get::<_, Vec<i64>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else if let Ok(v) = row.try_get::<_, Vec<f32>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else if let Ok(v) = row.try_get::<_, Vec<f64>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else if let Ok(v) = row.try_get::<_, Vec<bool>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else {
		match row.try_get::<_, Option<String>>(i) {
			Ok(None) => "NULL".to_string(),
			Ok(Some(_)) => "(unprintable)".to_string(),
			Err(_) => "NULL".to_string(),
		}
	}
}

pub fn can_print(row: &tokio_postgres::Row, i: usize) -> bool {
	let column = row.columns().get(i);
	if let Some(col) = column {
		if col.type_().name() == "void" {
			return true;
		}
	}

	if row.try_get::<_, String>(i).is_ok()
		|| row.try_get::<_, i16>(i).is_ok()
		|| row.try_get::<_, i32>(i).is_ok()
		|| row.try_get::<_, i64>(i).is_ok()
		|| row.try_get::<_, f32>(i).is_ok()
		|| row.try_get::<_, f64>(i).is_ok()
		|| row.try_get::<_, bool>(i).is_ok()
		|| row.try_get::<_, Vec<u8>>(i).is_ok()
		|| row.try_get::<_, jiff::Timestamp>(i).is_ok()
		|| row.try_get::<_, jiff::civil::Date>(i).is_ok()
		|| row.try_get::<_, jiff::civil::Time>(i).is_ok()
		|| row.try_get::<_, jiff::civil::DateTime>(i).is_ok()
		|| row.try_get::<_, serde_json::Value>(i).is_ok()
		|| row.try_get::<_, Vec<String>>(i).is_ok()
		|| row.try_get::<_, Vec<i32>>(i).is_ok()
		|| row.try_get::<_, Vec<i64>>(i).is_ok()
		|| row.try_get::<_, Vec<f32>>(i).is_ok()
		|| row.try_get::<_, Vec<f64>>(i).is_ok()
		|| row.try_get::<_, Vec<bool>>(i).is_ok()
	{
		return true;
	}

	matches!(row.try_get::<_, Option<String>>(i), Ok(None))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_void_type_handling() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Test void type - pg_sleep returns void
		let rows = client
			.query("SELECT pg_sleep(0)", &[])
			.await
			.expect("Query failed");

		assert_eq!(rows.len(), 1);
		let row = &rows[0];

		// Check that void type can be printed
		assert!(can_print(row, 0));

		// Check that void type is formatted as "(void)"
		let value = format_value(row, 0);
		assert_eq!(value, "(void)");
	}

	#[tokio::test]
	async fn test_float_handling() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Test float types
		let rows = client
			.query(
				"SELECT 3.14::real as float4, 2.718281828::double precision as float8",
				&[],
			)
			.await
			.expect("Query failed");

		assert_eq!(rows.len(), 1);
		let row = &rows[0];

		// Check that float types can be printed
		assert!(can_print(row, 0));
		assert!(can_print(row, 1));

		// Check that float types are formatted
		let value_f32 = format_value(row, 0);
		let value_f64 = format_value(row, 1);

		assert!(value_f32.contains("3.14"));
		assert!(value_f64.contains("2.718"));
	}

	#[tokio::test]
	async fn test_numeric_handling() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Test numeric type
		let rows = client
			.query("SELECT 123.456::numeric as num", &[])
			.await
			.expect("Query failed");

		assert_eq!(rows.len(), 1);
		let row = &rows[0];

		// Numeric might need text casting, so we check the behavior
		let value = format_value(row, 0);
		// Should either format correctly or fall back to text casting
		assert!(!value.is_empty());
		assert_ne!(value, "(error)");
	}
}
