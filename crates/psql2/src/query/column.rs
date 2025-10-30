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

	if let Some(ref text_rows) = text_rows {
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
