use std::net::SocketAddr;

use axum::{body::Body, extract::State, response::Response, routing::get, Router};
use clap::Parser;
use miette::{IntoDiagnostic, Result};
use opentelemetry::{
	global,
	metrics::{Counter, Gauge, UpDownCounter},
	KeyValue,
};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_semantic_conventions as semantics;
use prometheus::{Encoder, TextEncoder};
use reqwest::StatusCode;
use sysinfo::{Disks, Networks};
use tokio::net::TcpListener;
use tracing::{debug, info};

use super::Context;

/// Collect metrics as a daemon.
#[derive(Debug, Clone, Parser)]
pub struct ObserveArgs {}

pub async fn run(ctx: Context<ObserveArgs>) -> Result<()> {
	let app = Router::new()
		.route("/metrics", get(metrics))
		.with_state(AppState::new()?);
	let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
	info!(?addr, "Listening on");

	let listener = TcpListener::bind(addr).await.into_diagnostic()?;

	axum::serve(listener, app.into_make_service())
		.await
		.into_diagnostic()?;

	Ok(())
}

#[derive(Clone)]
struct AppState {
	registry: prometheus::Registry,
	memory_usage: UpDownCounter<i64>,
	memory_utilization: Gauge<f64>,
	disk_io: Counter<u64>,
	network_packets: Counter<u64>,
	network_errors: Counter<u64>,
	network_io: Counter<u64>,
}

impl AppState {
	fn new() -> Result<AppState> {
		let registry = prometheus::Registry::new();
		let metric_reader = opentelemetry_prometheus::exporter()
			.with_registry(registry.clone())
			.build()
			.into_diagnostic()?;

		let metrics = SdkMeterProvider::builder()
			.with_reader(metric_reader)
			.build();
		opentelemetry::global::set_meter_provider(metrics.clone());

		let meter = global::meter("bestool");

		let memory_usage = meter
			.i64_up_down_counter(semantics::metric::SYSTEM_MEMORY_USAGE)
			.with_unit("By")
			.build();

		let memory_utilization = meter
			.f64_gauge(semantics::metric::SYSTEM_MEMORY_UTILIZATION)
			.with_unit("1")
			.build();

		let disk_io = meter
			.u64_counter(semantics::metric::SYSTEM_DISK_IO)
			.with_unit("By")
			.build();

		let network_packets = meter
			.u64_counter(semantics::metric::SYSTEM_NETWORK_PACKETS)
			.with_unit("{packet}")
			.build();

		let network_errors = meter
			.u64_counter(semantics::metric::SYSTEM_NETWORK_ERRORS)
			.with_unit("{error}")
			.build();

		let network_io = meter
			.u64_counter(semantics::metric::SYSTEM_NETWORK_IO)
			.with_unit("By")
			.build();

		Ok(AppState {
			registry,
			memory_usage,
			memory_utilization,
			disk_io,
			network_packets,
			network_errors,
			network_io,
		})
	}
}

async fn metrics(
	State(AppState {
		registry,
		memory_usage,
		memory_utilization,
		disk_io,
		network_packets,
		network_errors,
		network_io,
	}): State<AppState>,
) -> Result<Response, (StatusCode, String)> {
	debug!("get metrics");
	let sysinfo = sysinfo::System::new_all();

	let used = sysinfo.used_memory();
	memory_usage.add(
		used as _,
		&[KeyValue::new(
			semantics::attribute::SYSTEM_MEMORY_STATE,
			"used",
		)],
	);
	let utilization = used as f64 / sysinfo.total_memory() as f64 * 100.0;
	memory_utilization.record(utilization, &[]);

	for disk in Disks::new_with_refreshed_list().list() {
		let usage = disk.usage();
		let direction_read = KeyValue::new(semantics::attribute::DISK_IO_DIRECTION, "read");
		let direction_write = KeyValue::new(semantics::attribute::DISK_IO_DIRECTION, "write");
		disk_io.add(
			usage.read_bytes,
			&[
				direction_read,
				KeyValue::new(
					semantics::attribute::DEVICE_ID,
					disk.name().to_string_lossy().into_owned(),
				),
			],
		);
		disk_io.add(
			usage.written_bytes,
			&[
				direction_write,
				KeyValue::new(
					semantics::attribute::DEVICE_ID,
					disk.name().to_string_lossy().into_owned(),
				),
			],
		);
	}

	for (name, network) in Networks::new_with_refreshed_list().list() {
		let direction_receive =
			KeyValue::new(semantics::attribute::NETWORK_IO_DIRECTION, "receive");
		let direction_transmit =
			KeyValue::new(semantics::attribute::NETWORK_IO_DIRECTION, "transmit");
		let device = KeyValue::new(semantics::attribute::SYSTEM_DEVICE, name.clone());
		let interface_name =
			KeyValue::new(semantics::attribute::NETWORK_INTERFACE_NAME, name.clone());
		network_packets.add(
			network.packets_received(),
			&[direction_receive.clone(), device.clone()],
		);
		network_packets.add(
			network.packets_transmitted(),
			&[direction_transmit.clone(), device],
		);
		network_errors.add(
			network.errors_on_received(),
			&[direction_receive.clone(), interface_name.clone()],
		);
		network_errors.add(
			network.errors_on_transmitted(),
			&[direction_transmit.clone(), interface_name.clone()],
		);
		network_io.add(
			network.received(),
			&[direction_receive, interface_name.clone()],
		);
		network_io.add(network.transmitted(), &[direction_transmit, interface_name]);
	}

	let encoder = TextEncoder::new();
	let metric_families = registry.gather();

	let mut buf = Vec::new();
	let res = encoder.encode(&metric_families, &mut buf);
	if let Err(e) = res {
		return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
	}

	Ok(Response::new(Body::from(buf)))
}
