pub fn get_json_value(
	row: &tokio_postgres::Row,
	column_index: usize,
	column: &tokio_postgres::Column,
) -> Option<serde_json::Value> {
	use serde_json::Value;

	if row.try_get::<_, Option<String>>(column_index).ok()? == None {
		return Some(Value::Null);
	}

	if let Ok(v) = row.try_get::<_, String>(column_index) {
		Some(Value::String(v))
	} else if let Ok(v) = row.try_get::<_, i16>(column_index) {
		Some(Value::Number(v.into()))
	} else if let Ok(v) = row.try_get::<_, i32>(column_index) {
		Some(Value::Number(v.into()))
	} else if let Ok(v) = row.try_get::<_, i64>(column_index) {
		Some(Value::Number(v.into()))
	} else if let Ok(v) = row.try_get::<_, f32>(column_index) {
		serde_json::Number::from_f64(v as f64).map(Value::Number)
	} else if let Ok(v) = row.try_get::<_, f64>(column_index) {
		serde_json::Number::from_f64(v).map(Value::Number)
	} else if let Ok(v) = row.try_get::<_, bool>(column_index) {
		Some(Value::Bool(v))
	} else if let Ok(v) = row.try_get::<_, Vec<u8>>(column_index) {
		Some(Value::String(format!("\\x{}", hex::encode(v))))
	} else if let Ok(v) = row.try_get::<_, jiff::Timestamp>(column_index) {
		Some(Value::String(v.to_string()))
	} else if let Ok(v) = row.try_get::<_, jiff::civil::Date>(column_index) {
		Some(Value::String(v.to_string()))
	} else if let Ok(v) = row.try_get::<_, jiff::civil::Time>(column_index) {
		Some(Value::String(v.to_string()))
	} else if let Ok(v) = row.try_get::<_, jiff::civil::DateTime>(column_index) {
		Some(Value::String(v.to_string()))
	} else if let Ok(v) = row.try_get::<_, serde_json::Value>(column_index) {
		Some(v)
	} else if let Ok(v) = row.try_get::<_, Vec<String>>(column_index) {
		Some(Value::Array(v.into_iter().map(Value::String).collect()))
	} else if let Ok(v) = row.try_get::<_, Vec<i32>>(column_index) {
		Some(Value::Array(
			v.into_iter().map(|x| Value::Number(x.into())).collect(),
		))
	} else if let Ok(v) = row.try_get::<_, Vec<i64>>(column_index) {
		Some(Value::Array(
			v.into_iter().map(|x| Value::Number(x.into())).collect(),
		))
	} else if let Ok(v) = row.try_get::<_, Vec<f32>>(column_index) {
		Some(Value::Array(
			v.into_iter()
				.filter_map(|x| serde_json::Number::from_f64(x as f64).map(Value::Number))
				.collect(),
		))
	} else if let Ok(v) = row.try_get::<_, Vec<f64>>(column_index) {
		Some(Value::Array(
			v.into_iter()
				.filter_map(|x| serde_json::Number::from_f64(x).map(Value::Number))
				.collect(),
		))
	} else if let Ok(v) = row.try_get::<_, Vec<bool>>(column_index) {
		Some(Value::Array(v.into_iter().map(Value::Bool).collect()))
	} else {
		Some(Value::String(format!(
			"(unprintable: {})",
			column.type_().name()
		)))
	}
}

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
