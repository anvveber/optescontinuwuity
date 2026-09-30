use std::{sync::Arc, time::Duration};

use conduwuit::{
	Config, Server,
	log::{Log, LogLevelReloadHandles},
};
use futures::StreamExt;
use ruma::{UserId, device_id, user_id};
use serde_json::{Value, json};
use tokio::{sync::mpsc, time::timeout};

use crate::{
	Services,
	sending::{Destination, SendingEvent},
};

async fn inbox_count(services: &Services, user: &UserId) -> usize {
	services
		.users
		.get_to_device_events(user, device_id!("DEVICE"), None, None)
		.count()
		.await
}

async fn services() -> (tempfile::TempDir, Arc<Services>) {
	_ = rustls::crypto::ring::default_provider().install_default();
	let directory = tempfile::tempdir().unwrap();
	let config: Config = serde_json::from_value(json!({
		"server_name": "example.com",
		"database_path": directory.path(),
		"db_pool_affinity": false,
		"db_pool_workers": 1,
		"rocksdb_parallelism_threads": 1,
		"db_cache_capacity_mb": 8,
		"db_write_buffer_capacity_mb": 8,
		"sender_workers": 1,
	}))
	.unwrap();
	let server = Arc::new(Server::new(config, Some(tokio::runtime::Handle::current()), Log {
		reload: LogLevelReloadHandles::default(),
		capture: Arc::default(),
	}));
	(directory, Services::build(server).await.unwrap())
}

async fn register(services: &Services, id: &str, url: Option<&str>, users: Value) {
	let registration = json!({
		"id": id, "url": url, "as_token": format!("as_{id}"), "hs_token": format!("hs_{id}"),
		"sender_localpart": id, "namespaces": {"users": users}, "rate_limited": false,
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
}

async fn enqueue(services: &Services, user: &UserId, content: Value) {
	services
		.users
		.add_to_device_event(
			user_id!("@sender:remote.example"),
			user,
			device_id!("DEVICE"),
			"com.example.custom",
			content,
		)
		.await;
}

#[tokio::test]
async fn msc4203_cancelled_enqueue() { check_blocked_enqueue(true).await; }

#[tokio::test]
async fn msc4203_enqueue_respects_sync_cursor() { check_blocked_enqueue(false).await; }

async fn check_blocked_enqueue(cancel: bool) {
	let (_directory, services) = services().await;
	let guard = services.appservice.read().await;
	let writer_services = services.clone();
	let writer = services.server.runtime().spawn(async move {
		register(&writer_services, "blocked", Some("http://127.0.0.1:1"), json!([])).await;
	});

	timeout(Duration::from_secs(5), async {
		while !services
			.users
			.status(user_id!("@blocked:example.com"))
			.await
			.is_found()
		{
			tokio::task::yield_now().await;
		}
	})
	.await
	.unwrap();
	let (started, mut waiting) = mpsc::unbounded_channel();
	let producer_services = services.clone();
	let producer = services.server.runtime().spawn(async move {
		started.send(()).unwrap();
		enqueue(&producer_services, user_id!("@target:example.com"), json!({})).await;
	});
	waiting.recv().await.unwrap();
	let cursor = services.globals.current_count().unwrap();
	assert_eq!(inbox_count(&services, user_id!("@target:example.com")).await, 0);
	if cancel {
		producer.abort();
		assert!(producer.await.unwrap_err().is_cancelled());
		drop(guard);
		writer.await.unwrap();
		assert_eq!(inbox_count(&services, user_id!("@target:example.com")).await, 0);
		return;
	}
	drop(guard);
	writer.await.unwrap();
	producer.await.unwrap();
	let events = services
		.users
		.get_to_device_events(
			user_id!("@target:example.com"),
			device_id!("DEVICE"),
			Some(cursor),
			None,
		)
		.collect::<Vec<_>>()
		.await;
	assert_eq!(events.len(), 1);
	assert!(events[0].0 > cursor);
	services
		.users
		.remove_to_device_events(
			user_id!("@target:example.com"),
			device_id!("DEVICE"),
			Some(cursor),
		)
		.await;
	assert_eq!(inbox_count(&services, user_id!("@target:example.com")).await, 1);
}

#[tokio::test]
async fn msc4203_namespaces_and_sync_copies() {
	let (_directory, services) = services().await;
	for (id, regex, exclusive, url) in [
		("exclusive", "^@bridge_.*:example.com$", true, Some("http://127.0.0.1:1")),
		("inclusive", "^@regular:example.com$", false, Some("http://127.0.0.1:1")),
		("overlap", "^@bridge_", false, Some("http://127.0.0.1:1")),
		("disabled", "^@offline:example.com$", true, None),
	] {
		register(&services, id, url, json!([{"exclusive": exclusive, "regex": regex}])).await;
	}
	let ghost = user_id!("@bridge_alice:example.com");
	let bot = user_id!("@exclusive:example.com");
	let regular = user_id!("@regular:example.com");
	let offline = user_id!("@offline:example.com");
	let unmatched = user_id!("@unmatched:example.com");
	let remote = user_id!("@bridge_alice:remote.example");
	for user in [ghost, bot, regular, offline, unmatched, remote] {
		enqueue(&services, user, json!({})).await;
	}
	let mut recipients = Vec::new();
	for id in ["exclusive", "inclusive", "overlap", "disabled"] {
		let dest = Destination::Appservice(id.into());
		let queued = services
			.sending
			.db
			.queued_requests(&dest)
			.collect::<Vec<_>>()
			.await;
		for (_, event) in queued {
			let SendingEvent::ToDevice(bytes) = event else {
				panic!("expected to-device delivery")
			};
			let delivery: Value = serde_json::from_slice(&bytes).unwrap();
			recipients.push((
				id,
				delivery["event"]["to_user_id"].as_str().unwrap().to_owned(),
				delivery["delete_after_delivery"].as_bool().unwrap(),
			));
		}
	}
	recipients.sort();
	assert_eq!(recipients, [
		("exclusive", ghost.to_string(), true),
		("exclusive", bot.to_string(), true),
		("inclusive", regular.to_string(), false),
		("overlap", ghost.to_string(), false),
	]);
	for user in [ghost, bot, regular, offline, unmatched, remote] {
		assert_eq!(inbox_count(&services, user).await, 1);
	}
	services
		.users
		.remove_to_device_events(ghost, device_id!("DEVICE"), None)
		.await;
	assert_eq!(inbox_count(&services, ghost).await, 0);
	assert_eq!(services.sending.db.queued_appservices().count().await, 4);
}
