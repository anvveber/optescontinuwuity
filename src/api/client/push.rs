use axum::extract::State;
use conduwuit::{Err, Error, Event, Result, debug_warn, err, matrix::pdu::RawPduId};
use conduwuit_service::Services;
use futures::{StreamExt, pin_mut};
use ruma::{
	CanonicalJsonObject, CanonicalJsonValue, UserId,
	api::{
		client::push::{
			delete_pushrule, get_notifications, get_pushers, get_pushrule, get_pushrule_actions,
			get_pushrule_enabled, get_pushrules_all, get_pushrules_global_scope, set_pusher,
			set_pushrule, set_pushrule_actions, set_pushrule_enabled,
		},
		error::ErrorKind,
	},
	events::{
		GlobalAccountDataEventType,
		push_rules::{PushRulesEvent, PushRulesEventContent},
	},
	push::{
		InsertPushRuleError, PredefinedContentRuleId, PredefinedOverrideRuleId,
		RemovePushRuleError, Ruleset,
	},
};

use crate::{Ruma, client::is_ignored_pdu};

/// # `GET /_matrix/client/r0/pushrules/`
///
/// Retrieves the push rules event for this user.
pub(crate) async fn get_pushrules_all_route(
	State(services): State<crate::State>,
	body: Ruma<get_pushrules_all::v3::Request>,
) -> Result<get_pushrules_all::v3::Response> {
	let sender_user = body.sender_user();

	let Some(content_value) = services
		.account_data
		.get_global::<CanonicalJsonObject>(sender_user, GlobalAccountDataEventType::PushRules)
		.await
		.ok()
		.and_then(|event| event.get("content").cloned())
		.filter(CanonicalJsonValue::is_object)
	else {
		// user somehow has non-existent push rule event. recreate it and return server
		// default silently
		return recreate_push_rules_and_return(&services, sender_user).await;
	};

	let account_data_content =
		serde_json::from_value::<PushRulesEventContent>(content_value.into()).map_err(|e| {
			err!(Database(warn!("Invalid push rules account data event in database: {e}")))
		})?;

	let mut global_ruleset = account_data_content.global;

	// remove old deprecated mentions push rules as per MSC4210
	// and update the stored server default push rules
	#[allow(deprecated)]
	{
		use ruma::push::RuleKind::*;
		if global_ruleset
			.get(Override, PredefinedOverrideRuleId::ContainsDisplayName.as_str())
			.is_some()
			|| global_ruleset
				.get(Override, PredefinedOverrideRuleId::RoomNotif.as_str())
				.is_some()
			|| global_ruleset
				.get(Content, PredefinedContentRuleId::ContainsUserName.as_str())
				.is_some()
		{
			global_ruleset
				.remove(Override, PredefinedOverrideRuleId::ContainsDisplayName)
				.ok();
			global_ruleset
				.remove(Override, PredefinedOverrideRuleId::RoomNotif)
				.ok();
			global_ruleset
				.remove(Content, PredefinedContentRuleId::ContainsUserName)
				.ok();

			global_ruleset.update_with_server_default(Ruleset::server_default(sender_user));

			let ty = GlobalAccountDataEventType::PushRules;
			let event = PushRulesEvent::new(PushRulesEventContent::new(global_ruleset.clone()));

			services
				.account_data
				.update(None, sender_user, ty.to_string().into(), &serde_json::to_value(event)?)
				.await?;
		}
	};

	Ok(get_pushrules_all::v3::Response::new(global_ruleset))
}

/// # `GET /_matrix/client/r0/pushrules/global/`
///
/// Retrieves the push rules event for this user.
///
/// This appears to be the exact same as `GET /_matrix/client/r0/pushrules/`.
pub(crate) async fn get_pushrules_global_route(
	State(services): State<crate::State>,
	body: Ruma<get_pushrules_global_scope::v3::Request>,
) -> Result<get_pushrules_global_scope::v3::Response> {
	let sender_user = body.sender_user();

	let Some(content_value) = services
		.account_data
		.get_global::<CanonicalJsonObject>(sender_user, GlobalAccountDataEventType::PushRules)
		.await
		.ok()
		.and_then(|event| event.get("content").cloned())
		.filter(CanonicalJsonValue::is_object)
	else {
		// user somehow has non-existent push rule event. recreate it and return server
		// default silently

		let global_ruleset = Ruleset::server_default(sender_user);
		let event = PushRulesEvent::new(PushRulesEventContent::new(global_ruleset.clone()));

		services
			.account_data
			.update(
				None,
				sender_user,
				GlobalAccountDataEventType::PushRules.to_string().into(),
				&serde_json::to_value(event)?,
			)
			.await?;

		return Ok(get_pushrules_global_scope::v3::Response::new(global_ruleset));
	};

	let account_data_content =
		serde_json::from_value::<PushRulesEventContent>(content_value.into()).map_err(|e| {
			err!(Database(warn!("Invalid push rules account data event in database: {e}")))
		})?;

	let mut global_ruleset = account_data_content.global;

	// remove old deprecated mentions push rules as per MSC4210
	// and update the stored server default push rules
	#[allow(deprecated)]
	{
		use ruma::push::RuleKind::*;
		if global_ruleset
			.get(Override, PredefinedOverrideRuleId::ContainsDisplayName.as_str())
			.is_some()
			|| global_ruleset
				.get(Override, PredefinedOverrideRuleId::RoomNotif.as_str())
				.is_some()
			|| global_ruleset
				.get(Content, PredefinedContentRuleId::ContainsUserName.as_str())
				.is_some()
		{
			global_ruleset
				.remove(Override, PredefinedOverrideRuleId::ContainsDisplayName)
				.ok();
			global_ruleset
				.remove(Override, PredefinedOverrideRuleId::RoomNotif)
				.ok();
			global_ruleset
				.remove(Content, PredefinedContentRuleId::ContainsUserName)
				.ok();

			global_ruleset.update_with_server_default(Ruleset::server_default(sender_user));

			services
				.account_data
				.update(
					None,
					sender_user,
					GlobalAccountDataEventType::PushRules.to_string().into(),
					&serde_json::to_value(PushRulesEvent::new(PushRulesEventContent::new(
						global_ruleset.clone(),
					)))
					.expect("to json always works"),
				)
				.await?;
		}
	};

	Ok(get_pushrules_global_scope::v3::Response::new(global_ruleset))
}

/// # `GET /_matrix/client/r0/pushrules/{scope}/{kind}/{ruleId}`
///
/// Retrieves a single specified push rule for this user.
pub(crate) async fn get_pushrule_route(
	State(services): State<crate::State>,
	body: Ruma<get_pushrule::v3::Request>,
) -> Result<get_pushrule::v3::Response> {
	let sender_user = body.sender_user.as_ref().expect("user is authenticated");

	// remove old deprecated mentions push rules as per MSC4210
	#[allow(deprecated)]
	if body.rule_id.as_str() == PredefinedContentRuleId::ContainsUserName.as_str()
		|| body.rule_id.as_str() == PredefinedOverrideRuleId::ContainsDisplayName.as_str()
		|| body.rule_id.as_str() == PredefinedOverrideRuleId::RoomNotif.as_str()
	{
		return Err!(Request(NotFound("Push rule not found.")));
	}

	let event: PushRulesEvent = services
		.account_data
		.get_global(sender_user, GlobalAccountDataEventType::PushRules)
		.await
		.map_err(|_| err!(Request(NotFound("PushRules event not found."))))?;

	let rule = event
		.content
		.global
		.get(body.kind.clone(), &body.rule_id)
		.map(Into::into);

	if let Some(rule) = rule {
		Ok(get_pushrule::v3::Response::new(rule))
	} else {
		Err!(Request(NotFound("Push rule not found.")))
	}
}

/// # `PUT /_matrix/client/r0/pushrules/global/{kind}/{ruleId}`
///
/// Creates a single specified push rule for this user.
pub(crate) async fn set_pushrule_route(
	State(services): State<crate::State>,
	body: Ruma<set_pushrule::v3::Request>,
) -> Result<set_pushrule::v3::Response> {
	let sender_user = body.sender_user();
	let body = &body.body;
	let mut account_data: PushRulesEvent = services
		.account_data
		.get_global(sender_user, GlobalAccountDataEventType::PushRules)
		.await
		.map_err(|_| err!(Request(NotFound("PushRules event not found."))))?;

	if let Err(error) = account_data.content.global.insert(
		body.rule.clone(),
		body.after.as_deref(),
		body.before.as_deref(),
	) {
		let err = match error {
			| InsertPushRuleError::ServerDefaultRuleId => Error::BadRequest(
				ErrorKind::InvalidParam,
				"Rule IDs starting with a dot are reserved for server-default rules.",
			),
			| InsertPushRuleError::InvalidRuleId => Error::BadRequest(
				ErrorKind::InvalidParam,
				"Rule ID containing invalid characters.",
			),
			| InsertPushRuleError::RelativeToServerDefaultRule => Error::BadRequest(
				ErrorKind::InvalidParam,
				"Can't place a push rule relatively to a server-default rule.",
			),
			| InsertPushRuleError::UnknownRuleId => Error::BadRequest(
				ErrorKind::NotFound,
				"The before or after rule could not be found.",
			),
			| InsertPushRuleError::BeforeHigherThanAfter => Error::BadRequest(
				ErrorKind::InvalidParam,
				"The before rule has a higher priority than the after rule.",
			),
			| _ => Error::BadRequest(ErrorKind::InvalidParam, "Invalid data."),
		};

		return Err(err);
	}

	let ty = GlobalAccountDataEventType::PushRules;
	services
		.account_data
		.update(None, sender_user, ty.to_string().into(), &serde_json::to_value(account_data)?)
		.await?;

	Ok(set_pushrule::v3::Response::new())
}

/// # `GET /_matrix/client/r0/pushrules/global/{kind}/{ruleId}/actions`
///
/// Gets the actions of a single specified push rule for this user.
pub(crate) async fn get_pushrule_actions_route(
	State(services): State<crate::State>,
	body: Ruma<get_pushrule_actions::v3::Request>,
) -> Result<get_pushrule_actions::v3::Response> {
	let sender_user = body.sender_user();

	// remove old deprecated mentions push rules as per MSC4210
	#[allow(deprecated)]
	if body.rule_id.as_str() == PredefinedContentRuleId::ContainsUserName.as_str()
		|| body.rule_id.as_str() == PredefinedOverrideRuleId::ContainsDisplayName.as_str()
		|| body.rule_id.as_str() == PredefinedOverrideRuleId::RoomNotif.as_str()
	{
		return Err!(Request(NotFound("Push rule not found.")));
	}

	let event: PushRulesEvent = services
		.account_data
		.get_global(sender_user, GlobalAccountDataEventType::PushRules)
		.await
		.map_err(|_| err!(Request(NotFound("PushRules event not found."))))?;

	let actions = event
		.content
		.global
		.get(body.kind.clone(), &body.rule_id)
		.map(|rule| rule.actions().to_owned())
		.ok_or_else(|| err!(Request(NotFound("Push rule not found."))))?;

	Ok(get_pushrule_actions::v3::Response::new(actions))
}

/// # `PUT /_matrix/client/r0/pushrules/global/{kind}/{ruleId}/actions`
///
/// Sets the actions of a single specified push rule for this user.
pub(crate) async fn set_pushrule_actions_route(
	State(services): State<crate::State>,
	body: Ruma<set_pushrule_actions::v3::Request>,
) -> Result<set_pushrule_actions::v3::Response> {
	let sender_user = body.sender_user();

	let mut account_data: PushRulesEvent = services
		.account_data
		.get_global(sender_user, GlobalAccountDataEventType::PushRules)
		.await
		.map_err(|_| err!(Request(NotFound("PushRules event not found."))))?;

	if account_data
		.content
		.global
		.set_actions(body.kind.clone(), &body.rule_id, body.actions.clone())
		.is_err()
	{
		return Err!(Request(NotFound("Push rule not found.")));
	}

	let ty = GlobalAccountDataEventType::PushRules;
	services
		.account_data
		.update(None, sender_user, ty.to_string().into(), &serde_json::to_value(account_data)?)
		.await?;

	Ok(set_pushrule_actions::v3::Response::new())
}

/// # `GET /_matrix/client/r0/pushrules/global/{kind}/{ruleId}/enabled`
///
/// Gets the enabled status of a single specified push rule for this user.
pub(crate) async fn get_pushrule_enabled_route(
	State(services): State<crate::State>,
	body: Ruma<get_pushrule_enabled::v3::Request>,
) -> Result<get_pushrule_enabled::v3::Response> {
	let sender_user = body.sender_user();

	// remove old deprecated mentions push rules as per MSC4210
	#[allow(deprecated)]
	if body.rule_id.as_str() == PredefinedContentRuleId::ContainsUserName.as_str()
		|| body.rule_id.as_str() == PredefinedOverrideRuleId::ContainsDisplayName.as_str()
		|| body.rule_id.as_str() == PredefinedOverrideRuleId::RoomNotif.as_str()
	{
		return Ok(get_pushrule_enabled::v3::Response::new(false));
	}

	let event: PushRulesEvent = services
		.account_data
		.get_global(sender_user, GlobalAccountDataEventType::PushRules)
		.await
		.map_err(|_| err!(Request(NotFound("PushRules event not found."))))?;

	let enabled = event
		.content
		.global
		.get(body.kind.clone(), &body.rule_id)
		.map(ruma::push::AnyPushRuleRef::enabled)
		.ok_or_else(|| err!(Request(NotFound("Push rule not found."))))?;

	Ok(get_pushrule_enabled::v3::Response::new(enabled))
}

/// # `PUT /_matrix/client/r0/pushrules/global/{kind}/{ruleId}/enabled`
///
/// Sets the enabled status of a single specified push rule for this user.
pub(crate) async fn set_pushrule_enabled_route(
	State(services): State<crate::State>,
	body: Ruma<set_pushrule_enabled::v3::Request>,
) -> Result<set_pushrule_enabled::v3::Response> {
	let sender_user = body.sender_user();

	let mut account_data: PushRulesEvent = services
		.account_data
		.get_global(sender_user, GlobalAccountDataEventType::PushRules)
		.await
		.map_err(|_| err!(Request(NotFound("PushRules event not found."))))?;

	if account_data
		.content
		.global
		.set_enabled(body.kind.clone(), &body.rule_id, body.enabled)
		.is_err()
	{
		return Err!(Request(NotFound("Push rule not found.")));
	}

	let ty = GlobalAccountDataEventType::PushRules;
	services
		.account_data
		.update(None, sender_user, ty.to_string().into(), &serde_json::to_value(account_data)?)
		.await?;

	Ok(set_pushrule_enabled::v3::Response::new())
}

/// # `DELETE /_matrix/client/r0/pushrules/global/{kind}/{ruleId}`
///
/// Deletes a single specified push rule for this user.
pub(crate) async fn delete_pushrule_route(
	State(services): State<crate::State>,
	body: Ruma<delete_pushrule::v3::Request>,
) -> Result<delete_pushrule::v3::Response> {
	let sender_user = body.sender_user();

	let mut account_data: PushRulesEvent = services
		.account_data
		.get_global(sender_user, GlobalAccountDataEventType::PushRules)
		.await
		.map_err(|_| err!(Request(NotFound("PushRules event not found."))))?;

	if let Err(error) = account_data
		.content
		.global
		.remove(body.kind.clone(), &body.rule_id)
	{
		let err = match error {
			| RemovePushRuleError::ServerDefault => Error::BadRequest(
				ErrorKind::InvalidParam,
				"Cannot delete a server-default pushrule.",
			),
			| RemovePushRuleError::NotFound =>
				Error::BadRequest(ErrorKind::NotFound, "Push rule not found."),
			| _ => Error::BadRequest(ErrorKind::InvalidParam, "Invalid data."),
		};

		return Err(err);
	}

	let ty = GlobalAccountDataEventType::PushRules;
	services
		.account_data
		.update(None, sender_user, ty.to_string().into(), &serde_json::to_value(account_data)?)
		.await?;

	Ok(delete_pushrule::v3::Response::new())
}

/// # `GET /_matrix/client/r0/pushers`
///
/// Gets all currently active pushers for the sender user.
pub(crate) async fn get_pushers_route(
	State(services): State<crate::State>,
	body: Ruma<get_pushers::v3::Request>,
) -> Result<get_pushers::v3::Response> {
	let sender_user = body.sender_user();

	Ok(get_pushers::v3::Response::new(services.pusher.get_pushers(sender_user).await))
}

/// # `GET /_matrix/client/v3/notifications`
///
/// Lists notification events for the sender user.
pub(crate) async fn get_notifications_route(
	State(services): State<crate::State>,
	body: Ruma<get_notifications::v3::Request>,
) -> Result<get_notifications::v3::Response> {
	let sender_user = body.sender_user();
	let from = body
		.from
		.as_deref()
		.map(str::parse::<u64>)
		.transpose()
		.map_err(|_| Error::BadRequest(ErrorKind::InvalidParam, "Invalid `from` token."))?;
	let limit = body
		.limit
		.map_or(50, |limit| usize::try_from(limit).unwrap_or(50));
	let highlights_only = body.only.as_deref() == Some("highlight");

	let mut notifications = Vec::with_capacity(limit);
	let mut next_token = None;
	let items = services.rooms.user.notifications_rev(sender_user, from);

	pin_mut!(items);
	while let Some((count, notification)) = items.next().await {
		let is_highlight = notification.actions.iter().any(|action| {
			matches!(
				action,
				ruma::push::Action::SetTweak(ruma::push::Tweak::Highlight(
					ruma::push::HighlightTweakValue::Yes,
				))
			)
		});

		if highlights_only && !is_highlight {
			continue;
		}

		let Some(notification) =
			build_notification(&services, sender_user, count, notification).await?
		else {
			continue;
		};

		notifications.push(notification);

		if notifications.len() == limit {
			next_token = count.checked_sub(1).map(|count| count.to_string());
			break;
		}
	}

	let mut response = get_notifications::v3::Response::new(notifications);
	response.next_token = next_token;
	Ok(response)
}

async fn build_notification(
	services: &Services,
	sender_user: &UserId,
	count: u64,
	notification: conduwuit_service::rooms::user::NotificationRecord,
) -> Result<Option<get_notifications::v3::Notification>> {
	let pdu_id = RawPduId::from(notification.pdu_id.as_slice());
	let mut pdu = match services.rooms.timeline.get_pdu_from_id(&pdu_id).await {
		| Ok(pdu) => pdu,
		| Err(e) => {
			debug_warn!("Failed to load notification PDU: {e}");
			return Ok(None);
		},
	};

	let room_id = pdu.room_id_or_hash();
	let read_up_to = services
		.rooms
		.user
		.notification_read_count(sender_user, &room_id, notification.thread_root.as_deref())
		.await;

	if !services
		.rooms
		.state_accessor
		.user_can_see_event(sender_user, &room_id, pdu.event_id())
		.await
	{
		return Ok(None);
	}

	if is_ignored_pdu(services, &pdu, sender_user)
		.await
		.unwrap_or(true)
	{
		return Ok(None);
	}

	if let Err(e) = services
		.rooms
		.pdu_metadata
		.add_bundled_aggregations_to_pdu(sender_user, &mut pdu)
		.await
	{
		debug_warn!("Failed to add bundled aggregations to notification event: {e}");
	}

	let ts = pdu.origin_server_ts();
	pdu.set_unsigned(Some(sender_user));

	Ok(Some(get_notifications::v3::Notification::new(
		notification.actions,
		pdu.into_format(),
		count <= read_up_to,
		room_id,
		ts,
	)))
}

/// # `POST /_matrix/client/r0/pushers/set`
///
/// Adds a pusher for the sender user.
///
/// - TODO: Handle `append`
pub(crate) async fn set_pushers_route(
	State(services): State<crate::State>,
	body: Ruma<set_pusher::v3::Request>,
) -> Result<set_pusher::v3::Response> {
	let sender_user = body.sender_user();

	services
		.pusher
		.set_pusher(sender_user, body.sender_device(), &body.action)
		.await?;

	Ok(set_pusher::v3::Response::new())
}

/// user somehow has bad push rules, these must always exist per spec.
/// so recreate it and return server default silently
pub async fn recreate_push_rules_and_return(
	services: &Services,
	sender_user: &UserId,
) -> Result<get_pushrules_all::v3::Response> {
	let global_ruleset = Ruleset::server_default(sender_user);
	let event = PushRulesEvent::new(PushRulesEventContent::new(global_ruleset.clone()));

	services
		.account_data
		.update(
			None,
			sender_user,
			GlobalAccountDataEventType::PushRules.to_string().into(),
			&serde_json::to_value(event)?,
		)
		.await?;

	Ok(get_pushrules_all::v3::Response::new(global_ruleset))
}
