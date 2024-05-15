#![cfg(target_os = "linux")]
//! An implementation of [improv-wifi] for Linux.
//!
//! This crate provides an implementation of the Improv Wi-Fi configuration protocol, as a
//! peripheral, via BlueZ's D-Bus API. It is intended to be used in conjunction with the
//! [improv-wifi] tooling for Web and Android, to allow for easy connection of an embedded Linux
//! device to a Wi-Fi network without the need for a display or other input peripherals.
//!
//! As there are many different network systems available for Linux, this crate supplies a trait to
//! allow any network configuration framework to be supported.
//!
//! # Examples
//!
//! # Features
//!
//! - `miette`: Implements `miette::Diagnostic` on the error type.
//! - `networkmanager`: Enables the `NetworkManager` wifi configurator.
//!
//! [improv-wifi]: https://www.improv-wifi.com
//! [NetworkManager]: https://www.networkmanager.dev

use std::marker::PhantomData;

use bluer::{
	gatt::local::{
		service_control, Application, ApplicationHandle, CharacteristicControl, Service,
		ServiceControl,
	},
	Adapter, Result, Uuid,
};
use characteristics::{capabilities::Capabilities, current_state::{CurrentState, State}, error_state::ErrorState};

mod characteristics;

const SERVICE_UUID: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268000);

pub struct ImprovWifi<T> {
	handler: PhantomData<T>,
	app: ApplicationHandle,
	service: ServiceControl,
	capabilities: Capabilities,
	current_state: CurrentState,
	error_state: ErrorState,
	rpc_command: CharacteristicControl,
	rpc_result: CharacteristicControl,
}

pub trait WifiConfigurator {
	fn can_identify() -> bool;
}

impl<T: WifiConfigurator> ImprovWifi<T> {
	pub async fn install(adapter: &Adapter, initial_state: State) -> Result<Self> {
		let (service, service_handle) = service_control();
		let (capabilities, capabilities_char) = Capabilities::install(T::can_identify());
		let (current_state, current_state_char) = CurrentState::install(initial_state);
		let (error_state, error_state_char) = ErrorState::install();
		let (rpc_command, rpc_command_char) = characteristics::rpc_command::install();
		let (rpc_result, rpc_result_char) = characteristics::rpc_result::install();

		let app = Application {
			services: vec![Service {
				uuid: SERVICE_UUID,
				primary: true,
				characteristics: vec![
					capabilities_char,
					current_state_char,
					error_state_char,
					rpc_command_char,
					rpc_result_char,
				],
				control_handle: service_handle,
				..Default::default()
			}],
			..Default::default()
		};

		Ok(ImprovWifi {
			handler: PhantomData,
			app: adapter.serve_gatt_application(app).await?,
			service,
			capabilities,
			current_state,
			error_state,
			rpc_command,
			rpc_result,
		})
	}
}
