use std::{collections::BTreeMap, sync::Arc};

use axum::extract::State;
use conduwuit::{
	Config, Server,
	log::{Log, LogLevelReloadHandles},
};
use futures::StreamExt;
use ruma::{
	MilliSecondsSinceUnixEpoch,
	api::{
		client::to_device::send_event_to_device,
		federation::transactions::{
			edu::{DirectDeviceContent, Edu},
			send_transaction_message,
		},
	},
	device_id,
	serde::Raw,
	server_name,
	to_device::DeviceIdOrAllDevices,
	user_id,
};
use serde_json::{Value, json};
use service::{
	Services,
	sending::{Destination, SendingEvent},
};

use super::send_transaction_message_route;
use crate::{Ruma, client::send_event_to_device_route, router::ClientIdentity};

async fn services() -> (tempfile::TempDir, Arc<Services>) {
	_ = rustls::crypto::ring::default_provider().install_default();
	let directory = tempfile::tempdir().unwrap();
	let config: Config = serde_json::from_value(json!({
		"server_name": "example.com", "database_path": directory.path(),
		"db_pool_affinity": false, "db_pool_workers": 1, "rocksdb_parallelism_threads": 1,
		"db_cache_capacity_mb": 8, "db_write_buffer_capacity_mb": 8, "sender_workers": 1,
	}))
	.unwrap();
	let server = Arc::new(Server::new(config, Some(tokio::runtime::Handle::current()), Log {
		reload: LogLevelReloadHandles::default(),
		capture: Arc::default(),
	}));
	(directory, Services::build(server).await.unwrap())
}

#[tokio::test]
async fn msc4203_client_and_federation_device_targets() {
	let (_directory, services) = services().await;
	let registration = json!({
		"id": "bridge", "url": "http://127.0.0.1:1", "as_token": "bridge_as",
		"hs_token": "bridge_hs", "sender_localpart": "bridge", "rate_limited": false,
		"namespaces": {"users": [{"exclusive": true, "regex": "^@ghost:example.com$"}]},
		"receive_ephemeral": false,
	});
	services
		.appservice
		.register_appservice(
			&serde_json::from_value(registration.clone()).unwrap(),
			&registration.to_string(),
		)
		.await
		.unwrap();
	let ghost = user_id!("@ghost:example.com");
	services.users.create_shadow_account(ghost).await.unwrap();
	for device in [device_id!("DEVICE"), device_id!("OTHER")] {
		services
			.users
			.create_device(ghost, Some(device.to_owned()), None, None, None)
			.await
			.unwrap();
	}
	let (state, _guard) = service::state::create(services.clone());
	let mut expected = Vec::new();
	for target in ["DEVICE", "*"] {
		let target_device = if target == "*" {
			DeviceIdOrAllDevices::AllDevices
		} else {
			DeviceIdOrAllDevices::DeviceId(target.into())
		};
		let content = json!({"target": target, "custom": {"nested": [1, true, "two"]}});
		let messages = BTreeMap::from([(
			ghost.to_owned(),
			BTreeMap::from([(
				target_device,
				Raw::from_json(serde_json::value::to_raw_value(&content).unwrap()),
			)]),
		)]);
		for _ in 0..2 {
			send_event_to_device_route(State(state), Ruma {
				body: send_event_to_device::v3::Request::new_raw(
					"com.example.custom".into(),
					format!("client-{target}").into(),
					messages.clone(),
				),
				json_body: None,
				identity: ClientIdentity::User {
					sender_user: user_id!("@sender:example.com").to_owned(),
					sender_device: device_id!("SENDER").to_owned(),
				},
			})
			.await
			.unwrap();
		}
		let mut direct = DirectDeviceContent::new(
			user_id!("@sender:remote.example").to_owned(),
			"com.example.custom".into(),
			format!("message-{target}").into(),
		);
		direct.messages = messages;
		let edu = Raw::new(&Edu::DirectToDevice(direct)).unwrap();
		for attempt in [0, 0, 1] {
			let mut body = send_transaction_message::v1::Request::new(
				format!("federation-{target}-{attempt}").into(),
				server_name!("remote.example").to_owned(),
				MilliSecondsSinceUnixEpoch::now(),
			);
			body.edus.push(edu.clone());
			send_transaction_message_route(State(state), Ruma {
				body,
				json_body: None,
				identity: server_name!("remote.example").to_owned(),
			})
			.await
			.unwrap();
		}
		for sender in [user_id!("@sender:example.com"), user_id!("@sender:remote.example")] {
			for device in if target == "*" {
				vec!["DEVICE", "OTHER"]
			} else {
				vec![target]
			} {
				expected.push(json!({
					"to_user_id": ghost, "to_device_id": device, "sender": sender,
					"type": "com.example.custom", "content": content,
				}));
			}
		}
	}
	let dest = Destination::Appservice("bridge".into());
	let mut actual = services
		.sending
		.db
		.queued_requests(&dest)
		.map(|(_, event)| {
			let SendingEvent::ToDevice(bytes) = event else {
				panic!("expected to-device delivery")
			};
			let delivery: Value = serde_json::from_slice(&bytes).unwrap();
			assert_eq!(delivery["delete_after_delivery"], true);
			delivery["event"].clone()
		})
		.collect::<Vec<_>>()
		.await;
	actual.sort_by_key(Value::to_string);
	expected.sort_by_key(Value::to_string);
	assert_eq!(actual, expected);
	for (device, count) in [(device_id!("DEVICE"), 4), (device_id!("OTHER"), 2)] {
		assert_eq!(
			services
				.users
				.get_to_device_events(ghost, device, None, None)
				.collect::<Vec<_>>()
				.await
				.len(),
			count
		);
	}
}
