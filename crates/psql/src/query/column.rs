pub fn get_value(
	row: &tokio_postgres::Row,
	column_index: usize,
	unprintable_columns: &[usize],
) -> String {
	if !unprintable_columns.contains(&column_index) {
		return format_value(row, column_index);
	}

	// For unprintable columns without async context, show a placeholder
	// The actual text casting happens in the display layer which is async
	"(binary data)".to_string()
}

pub fn format_value(row: &tokio_postgres::Row, i: usize) -> String {
	// Check for void type first
	let column = row.columns().get(i);
	if let Some(col) = column
		&& col.type_().name() == "void"
	{
		return "(void)".to_string();
	}

	// Try numeric type with fraction crate
	if let Ok(v) = row.try_get::<_, fraction::Decimal>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, String>(i) {
		v
	} else if let Ok(v) = row.try_get::<_, i16>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, i32>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, i64>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, f32>(i) {
		format!("{}", v)
	} else if let Ok(v) = row.try_get::<_, f64>(i) {
		format!("{}", v)
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
		// Try to get as string - many types can be retrieved as text
		match row.try_get::<_, String>(i) {
			Ok(v) => v,
			Err(_) => match row.try_get::<_, Option<String>>(i) {
				Ok(None) => "NULL".to_string(),
				Ok(Some(v)) => v,
				Err(_) => "NULL".to_string(),
			},
		}
	}
}

pub fn can_print(row: &tokio_postgres::Row, i: usize) -> bool {
	// Check for void type
	let column = row.columns().get(i);
	if let Some(col) = column
		&& col.type_().name() == "void"
	{
		return true;
	}

	if row.try_get::<_, fraction::Decimal>(i).is_ok()
		|| row.try_get::<_, String>(i).is_ok()
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

		// Numeric type should now be directly printable with fraction crate
		assert!(can_print(row, 0));

		// Check that the value can be formatted
		let value = format_value(row, 0);
		assert!(!value.is_empty());
		assert_ne!(value, "(error)");
		assert!(value.contains("123.456"));
	}

	#[tokio::test]
	async fn test_numeric_arithmetic_with_text_cast() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Test numeric arithmetic with explicit text cast
		let rows = client
			.query("SELECT (12.34 + 37.28)::text as result", &[])
			.await
			.expect("Query failed");

		assert_eq!(rows.len(), 1);
		let row = &rows[0];

		// With text cast, it should be printable
		assert!(can_print(row, 0));

		// Should be able to format the result
		let value = format_value(row, 0);
		assert!(!value.is_empty());
		assert_ne!(value, "(error)");
		assert!(value.starts_with("49.6"));
	}

	#[tokio::test]
	async fn test_numeric_arithmetic_direct() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Test numeric arithmetic (the original failing case) - now should work directly
		let rows = client
			.query("SELECT 12.34 + 37.28", &[])
			.await
			.expect("Query failed");

		assert_eq!(rows.len(), 1);
		let row = &rows[0];

		// With fraction crate, numeric should be directly printable
		assert!(can_print(row, 0));

		// Should be able to format the result
		let value = format_value(row, 0);
		assert!(!value.is_empty());
		assert_ne!(value, "(error)");
		assert!(value.starts_with("49.6"));
	}

	#[tokio::test]
	async fn test_numeric_arithmetic_question_column() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Test numeric arithmetic with ?column? (this was failing in the REPL)
		let rows = client
			.query("SELECT 12.34 + 37.28", &[])
			.await
			.expect("Query failed");

		assert_eq!(rows.len(), 1);
		let row = &rows[0];

		// Verify the column name is ?column?
		assert_eq!(row.columns()[0].name(), "?column?");

		// With fraction crate, numeric should now be directly printable
		assert!(can_print(row, 0));

		// Should be able to format directly without text casting
		let value = format_value(row, 0);
		assert!(!value.is_empty());
		assert_ne!(value, "(error)");
		assert!(value.starts_with("49.6"));
	}
}
