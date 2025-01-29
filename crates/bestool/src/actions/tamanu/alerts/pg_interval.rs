use std::{error::Error, time::Duration};

use bytes::{BufMut, BytesMut};
use miette::Result;
use tokio_postgres::types::{IsNull, ToSql, Type};

#[derive(Debug)]
pub struct Interval(pub Duration);

impl ToSql for Interval {
	fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
		out.put_i64(self.0.as_micros().try_into().unwrap_or_default());
		out.put_i32(0);
		out.put_i32(0);
		Ok(IsNull::No)
	}

	fn accepts(ty: &Type) -> bool {
		matches!(*ty, Type::INTERVAL)
	}

	tokio_postgres::types::to_sql_checked!();
}
