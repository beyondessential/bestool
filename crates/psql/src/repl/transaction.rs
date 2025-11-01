#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransactionState {
	None,
	Idle,
	Active,
	Error,
}

impl TransactionState {
	/// Check the transaction state of a connection by querying from a separate monitoring connection
	pub async fn check(
		monitor_client: &tokio_postgres::Client,
		backend_pid: i32,
	) -> TransactionState {
		match monitor_client
			.query_one(
				"SELECT state, backend_xid::text FROM pg_stat_activity WHERE pid = $1",
				&[&backend_pid],
			)
			.await
		{
			Ok(row) => {
				let state: String = row.get(0);
				let backend_xid: Option<String> = row.get(1);

				if state == "idle in transaction (aborted)" {
					TransactionState::Error
				} else if state.starts_with("idle in transaction") {
					if backend_xid.is_some() && !backend_xid.as_ref().unwrap().is_empty() {
						TransactionState::Active
					} else {
						TransactionState::Idle
					}
				} else if state == "active" {
					match monitor_client
						.query_one(
							"SELECT xact_start, backend_xid::text FROM pg_stat_activity WHERE pid = $1",
							&[&backend_pid],
						)
						.await
					{
						Ok(row) => {
							let xact_start: Option<std::time::SystemTime> = row.get(0);
							let backend_xid: Option<String> = row.get(1);

							if xact_start.is_some() {
								if backend_xid.is_some()
									&& !backend_xid.as_ref().unwrap().is_empty()
								{
									TransactionState::Active
								} else {
									TransactionState::Idle
								}
							} else {
								TransactionState::None
							}
						}
						Err(_) => TransactionState::None,
					}
				} else {
					TransactionState::None
				}
			}
			Err(_) => TransactionState::None,
		}
	}
}
