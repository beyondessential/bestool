// Copied from https://docs.rs/crate/serde_postgres/latest/source
// which seems to be gone from github and used outdated dependencies.
// Copyright to the original authors (1aim), MIT+Apache-2.0 licensed.

use std::{collections::HashMap, error::Error, ops::Deref};

use chrono::{DateTime, Utc};
use tokio_postgres::{
	types::{FromSql, Type},
	Row,
};
use uuid::Uuid;

/// The raw bytes of a value, allowing "conversion" from any postgres type.
///
/// This type intentionally cannot be converted from `NULL`, and attempting to
/// do so will result in an error. Instead, use `Option<Raw>`.
pub struct Raw<'a>(pub &'a [u8]);

impl<'a> FromSql<'a> for Raw<'a> {
	fn from_sql(_ty: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn Error + Send + Sync>> {
		Ok(Raw(raw))
	}

	fn accepts(_ty: &Type) -> bool {
		true
	}
}

impl<'a> Deref for Raw<'a> {
	type Target = [u8];

	fn deref(&self) -> &Self::Target {
		self.0
	}
}

pub fn col_to_value(
	col: &tokio_postgres::Column,
	row: &tokio_postgres::Row,
	i: usize,
) -> serde_json::Value {
	use serde_json::Value;
	use tokio_postgres::types::Type;

	if let Ok(None) = row.try_get::<_, Option<crate::postgres_to_value::Raw<'_>>>(i) {
		return Value::Null;
	}

	match col.type_() {
		t if *t == Type::BOOL => Value::Bool(row.try_get(i).unwrap()),
		t if *t == Type::INT2 => {
			Value::Number(serde_json::Number::from(row.try_get::<_, i16>(i).unwrap()))
		}
		t if *t == Type::INT4 => {
			Value::Number(serde_json::Number::from(row.try_get::<_, i32>(i).unwrap()))
		}
		t if *t == Type::INT8 => {
			Value::Number(serde_json::Number::from(row.try_get::<_, i64>(i).unwrap()))
		}
		// TODO: BYTEA
		_ if row.try_get::<_, DateTime<Utc>>(i).is_ok() => {
			Value::String(row.try_get::<_, DateTime<Utc>>(i).unwrap().to_rfc3339())
		}
		_ if row.try_get::<_, Uuid>(i).is_ok() => {
			Value::String(row.try_get::<_, Uuid>(i).unwrap().to_string())
		}
		_ => Value::String(row.try_get(i).unwrap_or("(unknown)".into())),
	}
}

pub fn rows_to_value_map(rows: &[Row]) -> Vec<HashMap<String, serde_json::Value>> {
	rows.iter()
		.map(|row| {
			let mut map = HashMap::new();
			for (i, col) in row.columns().iter().enumerate() {
				map.insert(col.name().to_owned(), col_to_value(col, row, i));
			}
			map
		})
		.collect()
}
