use std::error::Error;

use bestool_postgres::pool::PgPool;
use tokio_postgres::types::{FromSql, Type};
use tracing::debug;

/// Raw bytes wrapper that can extract any PostgreSQL value
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct RawValue {
	pgtype: Type,
	bytes: Vec<u8>,
	null: bool,
}

impl<'a> FromSql<'a> for RawValue {
	fn from_sql(ty: &Type, val: &'a [u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
		Ok(RawValue {
			pgtype: ty.clone(),
			bytes: val.to_vec(),
			null: false,
		})
	}

	fn from_sql_null(ty: &Type) -> Result<Self, Box<dyn Error + Sync + Send>> {
		Ok(RawValue {
			pgtype: ty.clone(),
			bytes: Vec::new(),
			null: true,
		})
	}

	fn accepts(_ty: &Type) -> bool {
		// Accept any type
		true
	}
}

impl tokio_postgres::types::ToSql for RawValue {
	fn to_sql(
		&self,
		_ty: &Type,
		out: &mut tokio_postgres::types::private::BytesMut,
	) -> Result<tokio_postgres::types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
		out.extend_from_slice(&self.bytes);
		Ok(tokio_postgres::types::IsNull::No)
	}

	fn accepts(_ty: &Type) -> bool {
		true
	}

	fn to_sql_checked(
		&self,
		_ty: &Type,
		out: &mut tokio_postgres::types::private::BytesMut,
	) -> Result<tokio_postgres::types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
		self.to_sql(_ty, out)
	}
}

/// Represents a cell to be cast (row index, column index)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellRef {
	pub row_idx: usize,
	pub col_idx: usize,
}

/// On-demand text caster that converts values to text without re-querying
#[derive(Debug, Clone)]
pub struct TextCaster {
	pool: PgPool,
}

impl TextCaster {
	/// Create a new text caster
	pub fn new(pool: PgPool) -> Self {
		Self { pool }
	}

	/// Cast multiple values in batches of up to 100 at a time
	/// Returns a vector of results in the same order as the input cells
	pub async fn cast_batch(
		&self,
		rows: &[tokio_postgres::Row],
		cells: &[CellRef],
	) -> Vec<Result<String, Box<dyn std::error::Error + Send + Sync>>> {
		const BATCH_SIZE: usize = 100;

		let mut results = Vec::with_capacity(cells.len());
		let num_chunks = cells.len().div_ceil(BATCH_SIZE);

		debug!(
			"Batch casting {} cells in {} chunk(s) of up to {} cells each",
			cells.len(),
			num_chunks,
			BATCH_SIZE
		);

		for (chunk_idx, chunk) in cells.chunks(BATCH_SIZE).enumerate() {
			debug!(
				"Processing chunk {}/{} with {} cells",
				chunk_idx + 1,
				num_chunks,
				chunk.len()
			);
			let chunk_results = self.cast_chunk(rows, chunk).await;
			results.extend(chunk_results);
		}

		results
	}

	async fn cast_chunk(
		&self,
		rows: &[tokio_postgres::Row],
		cells: &[CellRef],
	) -> Vec<Result<String, Box<dyn std::error::Error + Send + Sync>>> {
		if cells.is_empty() {
			return Vec::new();
		}

		// Extract raw values and track which ones need casting
		let mut raw_values = Vec::with_capacity(cells.len());
		let mut needs_cast = Vec::new(); // indices of values that need actual casting

		for (idx, cell) in cells.iter().enumerate() {
			let result: Result<RawValue, _> = rows[cell.row_idx].try_get(cell.col_idx);
			match result {
				Ok(raw) => {
					if raw.null {
						raw_values.push(Ok(raw));
					} else {
						needs_cast.push(idx);
						raw_values.push(Ok(raw));
					}
				}
				Err(e) => raw_values.push(Err(e)),
			}
		}

		// If nothing needs casting, return early
		if needs_cast.is_empty() {
			return raw_values
				.into_iter()
				.map(|r| match r {
					Ok(raw) => {
						if raw.null {
							Ok("NULL".to_string())
						} else {
							Ok("(error)".to_string())
						}
					}
					Err(e) => Err(Box::new(e) as Box<dyn Error + Send + Sync>),
				})
				.collect();
		}

		// Build a single query that casts all values at once
		let client = match self.pool.get().await {
			Ok(c) => c,
			Err(_e) => {
				return (0..cells.len())
					.map(|_| {
						Err(
							Box::new(std::io::Error::other("Failed to get database connection"))
								as Box<dyn Error + Send + Sync>,
						)
					})
					.collect();
			}
		};

		// First, get all type names in one query
		let oids: Vec<u32> = needs_cast
			.iter()
			.filter_map(|&idx| {
				if let Ok(ref raw) = raw_values[idx] {
					Some(raw.pgtype.oid())
				} else {
					None
				}
			})
			.collect();

		let mut type_names = Vec::with_capacity(oids.len());
		for oid in oids {
			match client
				.query_one("SELECT typname FROM pg_type WHERE oid = $1", &[&oid])
				.await
			{
				Ok(row) => type_names.push(row.get::<_, String>(0)),
				Err(e) => {
					// If we can't get type name, return all errors
					return (0..cells.len())
						.map(|_| {
							Err(Box::new(std::io::Error::other(format!(
								"Failed to get type name: {}",
								e
							))) as Box<dyn Error + Send + Sync>)
						})
						.collect();
				}
			}
		}

		// Build the combined SELECT query
		let mut query = String::from("SELECT ");
		let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
		let mut cast_positions = Vec::new(); // Maps result column index to raw_values index

		for (param_idx, &raw_idx) in needs_cast.iter().enumerate() {
			if param_idx > 0 {
				query.push_str(", ");
			}

			if let Ok(ref raw) = raw_values[raw_idx] {
				let typename = &type_names[param_idx];
				query.push_str(&format!("${}::{}::text", param_idx + 1, typename));
				params.push(raw);
				cast_positions.push(raw_idx);
			}
		}

		debug!(
			"Executing batch cast query with {} parameters: {}",
			params.len(),
			query
		);

		// Execute the combined query
		let cast_results = match client.query_one(&query, &params).await {
			Ok(row) => {
				let mut results = Vec::new();
				for col_idx in 0..row.len() {
					match row.try_get::<_, String>(col_idx) {
						Ok(text) => results.push(Ok(text)),
						Err(e) => results.push(Err(Box::new(e) as Box<dyn Error + Send + Sync>)),
					}
				}
				results
			}
			Err(e) => {
				// If the batch query fails, return error for all cells that needed casting
				let error_msg = format!("Batch cast query failed: {}", e);
				(0..needs_cast.len())
					.map(|_| {
						Err(Box::new(std::io::Error::other(error_msg.clone()))
							as Box<dyn Error + Send + Sync>)
					})
					.collect()
			}
		};

		// Now build the final results vector in the original order
		let mut results = Vec::with_capacity(cells.len());
		let mut cast_iter = cast_results.into_iter();

		for (idx, raw_result) in raw_values.into_iter().enumerate() {
			if needs_cast.contains(&idx) {
				// This cell needed casting, get the next result
				results.push(cast_iter.next().unwrap_or_else(|| {
					Err(Box::new(std::io::Error::other("Missing cast result"))
						as Box<dyn Error + Send + Sync>)
				}));
			} else {
				// This cell was NULL or had an error
				match raw_result {
					Ok(raw) => {
						if raw.null {
							results.push(Ok("NULL".to_string()));
						} else {
							results.push(Ok("(unexpected)".to_string()));
						}
					}
					Err(e) => results.push(Err(Box::new(e) as Box<dyn Error + Send + Sync>)),
				}
			}
		}

		results
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn basic_int() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT 123::int4 as record_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		let result = &results[0];
		if let Err(e) = result {
			eprintln!("Error casting to text: {:?}", e);
		}
		assert!(
			result.is_ok(),
			"Failed to cast to text: {:?}",
			result.as_ref().err()
		);
		let text = result.as_ref().unwrap();
		eprintln!("Got text: {}", text);
		assert_eq!(text, "123", "Text doesn't match expected value: {}", text);
	}

	#[tokio::test]
	async fn batch_multiple_values() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query(
				"SELECT '$100.50'::money, '$200.75'::money UNION ALL SELECT '$300.25'::money, '$400.00'::money",
				&[],
			)
			.await
			.unwrap();

		let cells = vec![
			CellRef {
				row_idx: 0,
				col_idx: 0,
			},
			CellRef {
				row_idx: 0,
				col_idx: 1,
			},
			CellRef {
				row_idx: 1,
				col_idx: 0,
			},
			CellRef {
				row_idx: 1,
				col_idx: 1,
			},
		];

		let results = caster.cast_batch(&rows, &cells).await;

		assert_eq!(results.len(), 4);
		for result in &results {
			assert!(result.is_ok());
		}

		let values: Vec<String> = results.into_iter().map(|r| r.unwrap()).collect();
		assert!(values[0].contains("100"));
		assert!(values[1].contains("200"));
		assert!(values[2].contains("300"));
		assert!(values[3].contains("400"));
	}

	#[tokio::test]
	async fn batch_large_number() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		// Generate 150 rows to test chunking (should be split into 100 + 50)
		let rows = client
			.query(
				"SELECT generate_series(1, 150)::text::money as money_col",
				&[],
			)
			.await
			.unwrap();

		let cells: Vec<CellRef> = (0..150)
			.map(|i| CellRef {
				row_idx: i,
				col_idx: 0,
			})
			.collect();

		let results = caster.cast_batch(&rows, &cells).await;

		assert_eq!(results.len(), 150);
		for (i, result) in results.iter().enumerate() {
			assert!(
				result.is_ok(),
				"Failed to cast row {}: {:?}",
				i,
				result.as_ref().err()
			);
		}
	}

	#[tokio::test]
	async fn batch_query_verification() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		// Create a query with multiple different types
		let rows = client
			.query(
				"SELECT
					'$100.50'::money as col1,
					'$200.75'::money as col2,
					'$300.25'::money as col3
				UNION ALL SELECT
					'$400.00'::money,
					'$500.00'::money,
					'$600.00'::money",
				&[],
			)
			.await
			.unwrap();

		// Collect all 6 cells (2 rows × 3 columns)
		let cells: Vec<CellRef> = vec![
			CellRef {
				row_idx: 0,
				col_idx: 0,
			},
			CellRef {
				row_idx: 0,
				col_idx: 1,
			},
			CellRef {
				row_idx: 0,
				col_idx: 2,
			},
			CellRef {
				row_idx: 1,
				col_idx: 0,
			},
			CellRef {
				row_idx: 1,
				col_idx: 1,
			},
			CellRef {
				row_idx: 1,
				col_idx: 2,
			},
		];

		let results = caster.cast_batch(&rows, &cells).await;

		// All 6 values should be cast successfully
		assert_eq!(results.len(), 6);
		for result in &results {
			assert!(result.is_ok(), "Cast failed: {:?}", result);
		}

		// Verify the values contain the expected amounts
		let values: Vec<String> = results.into_iter().map(|r| r.unwrap()).collect();
		assert!(values[0].contains("100"));
		assert!(values[1].contains("200"));
		assert!(values[2].contains("300"));
		assert!(values[3].contains("400"));
		assert!(values[4].contains("500"));
		assert!(values[5].contains("600"));

		// The key point: this should have executed only 1 SELECT with 6 parameters,
		// not 6 separate SELECT queries. We verify this by the debug log output.
	}

	#[tokio::test]
	async fn batch_mixed_types() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		// Create a query with multiple different types in the same row
		let rows = client
			.query(
				"SELECT
					'$99.99'::money as money_col,
					'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'::uuid as uuid_col,
					point(3.14, 2.71) as point_col,
					'192.168.1.100'::inet as inet_col",
				&[],
			)
			.await
			.unwrap();

		// Collect all 4 cells (different types)
		let cells: Vec<CellRef> = vec![
			CellRef {
				row_idx: 0,
				col_idx: 0,
			}, // money
			CellRef {
				row_idx: 0,
				col_idx: 1,
			}, // uuid
			CellRef {
				row_idx: 0,
				col_idx: 2,
			}, // point
			CellRef {
				row_idx: 0,
				col_idx: 3,
			}, // inet
		];

		let results = caster.cast_batch(&rows, &cells).await;

		// All 4 values should be cast successfully
		assert_eq!(results.len(), 4);
		for (idx, result) in results.iter().enumerate() {
			assert!(result.is_ok(), "Cast {} failed: {:?}", idx, result);
		}

		// Verify the values
		let values: Vec<String> = results.into_iter().map(|r| r.unwrap()).collect();
		assert!(values[0].contains("99")); // money
		assert_eq!(values[1], "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"); // uuid
		assert!(values[2].contains("3.14") && values[2].contains("2.71")); // point
		assert!(values[3].contains("192.168.1.100")); // inet

		// The key point: this executes as a single query:
		// SELECT $1::money::text, $2::uuid::text, $3::point::text, $4::inet::text
	}

	#[tokio::test]
	#[ignore = "FIXME: figure out a workaround for (anonymous?) composite types"]
	async fn composite() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT row(1, 'test', true) as record_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		let result = &results[0];
		if let Err(e) = result {
			eprintln!("Error casting to text: {:?}", e);
		}
		assert!(
			result.is_ok(),
			"Failed to cast to text: {:?}",
			result.as_ref().err()
		);
		let text = result.as_ref().unwrap();
		eprintln!("Got text: {}", text);
		assert!(
			text.contains("1") && text.contains("test"),
			"Text doesn't contain expected values: {}",
			text
		);
	}

	#[tokio::test]
	async fn money_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT '$1,234.56'::money as money_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast money: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("Money as text: {}", text);
		assert!(text.contains("1") && text.contains("234"));
	}

	#[tokio::test]
	async fn uuid_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query(
				"SELECT 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'::uuid as uuid_col",
				&[],
			)
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast uuid: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("UUID as text: {}", text);
		assert_eq!(text, "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11");
	}

	#[tokio::test]
	async fn json_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT '{\"key\": \"value\"}'::json as json_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast json: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("JSON as text: {}", text);
		assert!(text.contains("key") && text.contains("value"));
	}

	#[tokio::test]
	async fn jsonb_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT '{\"foo\": \"bar\"}'::jsonb as jsonb_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast jsonb: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("JSONB as text: {}", text);
		assert!(text.contains("foo") && text.contains("bar"));
	}

	#[tokio::test]
	async fn array_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT ARRAY[1, 2, 3, 4]::int[] as array_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast array: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("Array as text: {}", text);
		assert!(text.contains("1") && text.contains("2") && text.contains("3"));
	}

	#[tokio::test]
	async fn bytea_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT '\\xDEADBEEF'::bytea as bytea_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast bytea: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("Bytea as text: {}", text);
		assert!(text.contains("\\x") || text.contains("de") || text.contains("ad"));
	}

	#[tokio::test]
	async fn inet_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT '192.168.1.1/24'::inet as inet_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast inet: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("Inet as text: {}", text);
		assert!(text.contains("192.168.1.1"));
	}

	#[tokio::test]
	async fn interval_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT '2 days 3 hours'::interval as interval_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast interval: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("Interval as text: {}", text);
		assert!(text.contains("day") || text.contains("hour") || text.contains("2"));
	}

	#[tokio::test]
	async fn null_value() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT NULL::int as null_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast null: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("NULL as text: {}", text);
		assert_eq!(text, "NULL");
	}

	#[tokio::test]
	async fn point_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT point(1.5, 2.5) as point_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast point: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("Point as text: {}", text);
		assert!(text.contains("1.5") && text.contains("2.5"));
	}

	#[tokio::test]
	async fn box_type() {
		let connection_string = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let caster = TextCaster::new(pool.clone());

		let client = pool.get().await.unwrap();
		let rows = client
			.query("SELECT box(point(0,0), point(1,1)) as box_col", &[])
			.await
			.unwrap();

		let cell = CellRef {
			row_idx: 0,
			col_idx: 0,
		};
		let results = caster.cast_batch(&rows, &[cell]).await;
		assert_eq!(results.len(), 1);
		assert!(
			results[0].is_ok(),
			"Failed to cast box: {:?}",
			results[0].as_ref().err()
		);
		let text = results[0].as_ref().unwrap();
		eprintln!("Box as text: {}", text);
		assert!(text.contains("0") && text.contains("1"));
	}
}
