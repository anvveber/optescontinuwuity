use std::collections::BTreeSet;

use conduwuit::{matrix::StateKey, utils::math::usize_from_ruma};
use ruma::{UInt, UserId, events::StateEventType};

const MAX_TIMELINE_LIMIT: usize = 100;

// The unstable MSC4186 endpoint ruma implements, which clients use today,
// sends `required_state` as `[event_type, state_key]` pairs with these
// wildcards. The merged proposal replaced them with `include`, `exclude` and
// `lazy_members`, which ruma does not model yet.
const WILDCARD: &str = "*";
const LAZY: &str = "$LAZY";
const ME: &str = "$ME";

/// How much of a room to send: its timeline limit and required state, combined
/// over every list and subscription selecting the room.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RoomConfig {
	pub timeline_limit: usize,
	pub required_state: RequiredState,
}

impl RoomConfig {
	#[must_use]
	pub fn new(required_state: &[(StateEventType, String)], timeline_limit: UInt) -> Self {
		Self {
			timeline_limit: usize_from_ruma(timeline_limit).min(MAX_TIMELINE_LIMIT),
			required_state: RequiredState::new(required_state),
		}
	}

	pub fn merge(&mut self, other: &Self) {
		self.timeline_limit = self.timeline_limit.max(other.timeline_limit);
		self.required_state.merge(&other.required_state);
	}

	#[must_use]
	pub fn expands(&self, previous: &Self) -> bool {
		self.timeline_limit > previous.timeline_limit
			|| !previous.required_state.covers(&self.required_state)
	}
}

/// The unstable `*`, `$ME` and `$LAZY` are resolved here.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequiredState {
	patterns: BTreeSet<StatePattern>,
	lazy_members: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct StatePattern {
	/// `None` matches every event type.
	event_type: Option<StateEventType>,
	state_key: StateKeyPattern,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum StateKeyPattern {
	Any,
	Me,
	Exact(StateKey),
}

impl RequiredState {
	fn new(required_state: &[(StateEventType, String)]) -> Self {
		let mut this = Self::default();
		for (event_type, state_key) in required_state {
			let event_type = (event_type.to_string() != WILDCARD).then(|| event_type.clone());
			let state_key = match state_key.as_str() {
				| WILDCARD => StateKeyPattern::Any,
				| ME => StateKeyPattern::Me,
				| LAZY if event_type == Some(StateEventType::RoomMember) => {
					this.lazy_members = true;
					continue;
				},
				| state_key => StateKeyPattern::Exact(state_key.into()),
			};

			this.patterns.insert(StatePattern { event_type, state_key });
		}

		this
	}

	/// Merges `other` into this, so it asks for the state either asks for.
	///
	/// Lazy loading stays off if either config returns every member, as
	/// [MSC4186] requires.
	///
	/// [MSC4186]: https://github.com/matrix-org/matrix-spec-proposals/pull/4186
	fn merge(&mut self, other: &Self) {
		let lazy_members = (self.lazy_members || other.lazy_members)
			&& !self.returns_all_members()
			&& !other.returns_all_members();

		self.patterns.extend(other.patterns.iter().cloned());
		self.lazy_members = lazy_members;
	}

	fn returns_all_members(&self) -> bool {
		!self.lazy_members
			&& self.patterns.iter().any(|pattern| {
				pattern.state_key == StateKeyPattern::Any
					&& pattern
						.event_type
						.as_ref()
						.is_none_or(|event_type| *event_type == StateEventType::RoomMember)
			})
	}

	fn covers(&self, other: &Self) -> bool {
		let lazy_covered = !other.lazy_members || self.lazy_members || self.returns_all_members();
		let members_covered = !other.returns_all_members() || self.returns_all_members();

		self.patterns.is_superset(&other.patterns) && lazy_covered && members_covered
	}

	#[must_use]
	pub fn lazy_members(&self) -> bool { self.lazy_members }

	#[must_use]
	pub fn has_wildcard(&self) -> bool {
		self.patterns.iter().any(|pattern| {
			pattern.event_type.is_none() || pattern.state_key == StateKeyPattern::Any
		})
	}

	pub fn exact_keys<'a>(
		&'a self,
		me: &'a UserId,
	) -> impl Iterator<Item = (&'a StateEventType, &'a str)> + Send + 'a {
		self.patterns.iter().filter_map(move |pattern| {
			match (&pattern.event_type, &pattern.state_key) {
				| (Some(event_type), StateKeyPattern::Exact(state_key)) =>
					Some((event_type, state_key.as_str())),
				| (Some(event_type), StateKeyPattern::Me) => Some((event_type, me.as_str())),
				| _ => None,
			}
		})
	}

	/// Returns whether a state event is asked for. With lazy loading, a type
	/// wildcard does not match memberships, as the lazy loading rules decide
	/// which of them are sent.
	#[must_use]
	pub fn matches(&self, event_type: &StateEventType, state_key: &str, me: &UserId) -> bool {
		let lazy_member = self.lazy_members && *event_type == StateEventType::RoomMember;

		self.patterns.iter().any(|pattern| {
			let wildcard_member = lazy_member && pattern.state_key == StateKeyPattern::Any;

			pattern
				.event_type
				.as_ref()
				.map_or(!wildcard_member, |expected| expected == event_type)
				&& match &pattern.state_key {
					| StateKeyPattern::Any => true,
					| StateKeyPattern::Me => state_key == me.as_str(),
					| StateKeyPattern::Exact(expected) => expected.as_str() == state_key,
				}
		})
	}
}

#[cfg(test)]
mod tests {
	use ruma::{uint, user_id};

	use super::*;

	fn config(required_state: &[(&str, &str)], timeline_limit: UInt) -> RoomConfig {
		let required_state: Vec<_> = required_state
			.iter()
			.map(|(event_type, state_key)| ((*event_type).into(), (*state_key).to_owned()))
			.collect();

		RoomConfig::new(&required_state, timeline_limit)
	}

	#[test]
	fn wildcards_are_resolved() {
		let me = user_id!("@me:example.com");
		let config = config(
			&[("m.room.name", ""), ("m.room.member", "$ME"), ("m.room.member", "$LAZY")],
			uint!(1),
		);
		let required_state = &config.required_state;

		assert!(required_state.lazy_members());
		assert!(!required_state.has_wildcard());
		assert!(required_state.matches(&StateEventType::RoomName, "", me));
		assert!(required_state.matches(&StateEventType::RoomMember, me.as_str(), me));
		assert!(!required_state.matches(&StateEventType::RoomMember, "@other:example.com", me));
		assert_eq!(required_state.exact_keys(me).count(), 2);
	}

	#[test]
	fn type_and_key_wildcards_match_everything_they_cover() {
		let me = user_id!("@me:example.com");
		let all = config(&[("*", "*")], uint!(0));
		let members = config(&[("m.room.member", "*")], uint!(0));

		assert!(all.required_state.has_wildcard());
		assert!(
			all.required_state
				.matches(&StateEventType::RoomTopic, "", me)
		);
		assert!(
			members
				.required_state
				.matches(&StateEventType::RoomMember, "@a:b.c", me)
		);
		assert!(
			!members
				.required_state
				.matches(&StateEventType::RoomTopic, "", me)
		);
	}

	#[test]
	fn timeline_limit_is_capped() {
		assert_eq!(config(&[], uint!(10_000)).timeline_limit, MAX_TIMELINE_LIMIT);
	}

	#[test]
	fn merge_takes_the_largest_limit_and_the_union_of_state() {
		let mut merged = config(&[("m.room.name", "")], uint!(1));
		merged.merge(&config(&[("m.room.topic", ""), ("m.room.member", "$LAZY")], uint!(20)));

		assert_eq!(merged.timeline_limit, 20);
		assert!(merged.required_state.lazy_members());
		assert_eq!(
			merged
				.required_state
				.exact_keys(user_id!("@me:example.com"))
				.count(),
			2
		);
	}

	#[test]
	fn lazy_loading_leaves_members_out_of_type_wildcards() {
		let me = user_id!("@me:example.com");
		let lazy = config(&[("*", "*"), ("m.room.member", "$LAZY")], uint!(1));

		assert!(
			lazy.required_state
				.matches(&StateEventType::RoomTopic, "", me)
		);
		assert!(
			!lazy
				.required_state
				.matches(&StateEventType::RoomMember, "@a:b.c", me)
		);
	}

	#[test]
	fn explicit_members_are_kept_under_lazy_loading() {
		let me = user_id!("@me:example.com");
		let lazy = config(&[("*", "$ME"), ("m.room.member", "$LAZY")], uint!(1));

		assert!(
			lazy.required_state
				.matches(&StateEventType::RoomMember, me.as_str(), me)
		);
	}

	#[test]
	fn turning_lazy_loading_off_or_on_is_detected_correctly() {
		let lazy = config(&[("*", "*"), ("m.room.member", "$LAZY")], uint!(1));
		let all = config(&[("*", "*")], uint!(1));

		assert!(all.expands(&lazy));
		assert!(!lazy.expands(&all));
	}

	#[test]
	fn lazy_loading_is_off_when_another_config_returns_every_member() {
		let mut merged = config(&[("m.room.member", "$LAZY")], uint!(1));
		merged.merge(&config(&[("m.room.member", "*")], uint!(1)));

		assert!(!merged.required_state.lazy_members());

		let mut merged = config(&[("m.room.member", "$LAZY")], uint!(1));
		merged.merge(&config(&[("m.room.name", "")], uint!(1)));

		assert!(merged.required_state.lazy_members());
	}

	#[test]
	fn expansion_is_detected_for_limits_and_state_only() {
		let previous = config(&[("m.room.name", "")], uint!(10));

		assert!(!config(&[("m.room.name", "")], uint!(10)).expands(&previous));
		assert!(!config(&[], uint!(1)).expands(&previous));
		assert!(config(&[("m.room.name", "")], uint!(11)).expands(&previous));
		assert!(config(&[("m.room.topic", "")], uint!(10)).expands(&previous));
		assert!(config(&[("m.room.member", "$LAZY")], uint!(1)).expands(&previous));
	}
}
