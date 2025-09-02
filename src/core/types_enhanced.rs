// Enhanced Type Definitions for Continuwuity
// Contributed from our chat-system improvements
// Provides cleaner, more focused type definitions for Matrix operations

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Enhanced room information with better organization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedRoomInfo {
    pub room_id: String,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub avatar_url: Option<String>,
    pub member_count: u32,
    pub is_public: bool,
    pub join_rule: JoinRule,
    pub power_levels: PowerLevels,
    pub creation_time: u64,
    pub last_activity: u64,
}

/// Enhanced user information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedUserInfo {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_online: bool,
    pub last_seen: Option<u64>,
    pub power_level: i32,
    pub membership_state: MembershipState,
}

/// Join rules for rooms
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JoinRule {
    #[serde(rename = "public")]
    Public,
    #[serde(rename = "invite")]
    Invite,
    #[serde(rename = "private")]
    Private,
    #[serde(rename = "knock")]
    Knock,
    #[serde(rename = "knock_restricted")]
    KnockRestricted,
}

/// Membership states
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MembershipState {
    #[serde(rename = "invite")]
    Invite,
    #[serde(rename = "join")]
    Join,
    #[serde(rename = "knock")]
    Knock,
    #[serde(rename = "leave")]
    Leave,
    #[serde(rename = "ban")]
    Ban,
}

/// Power levels for room permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerLevels {
    pub users: HashMap<String, i32>,
    pub events: HashMap<String, i32>,
    pub events_default: i32,
    pub state_default: i32,
    pub ban: i32,
    pub kick: i32,
    pub redact: i32,
    pub invite: i32,
}

impl Default for PowerLevels {
    fn default() -> Self {
        Self {
            users: HashMap::new(),
            events: HashMap::new(),
            events_default: 0,
            state_default: 50,
            ban: 50,
            kick: 50,
            redact: 50,
            invite: 0,
        }
    }
}

/// Enhanced message content with better structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedMessageContent {
    pub msgtype: MessageType,
    pub body: String,
    pub formatted_body: Option<String>,
    pub format: Option<String>,
    pub relates_to: Option<RelatesTo>,
    pub mentions: Option<Mentions>,
    pub thread: Option<ThreadInfo>,
}

/// Message types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageType {
    #[serde(rename = "m.text")]
    Text,
    #[serde(rename = "m.notice")]
    Notice,
    #[serde(rename = "m.emote")]
    Emote,
    #[serde(rename = "m.image")]
    Image,
    #[serde(rename = "m.file")]
    File,
    #[serde(rename = "m.video")]
    Video,
    #[serde(rename = "m.audio")]
    Audio,
    #[serde(rename = "m.location")]
    Location,
}

/// Relates to information for replies/threads
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatesTo {
    #[serde(rename = "m.in_reply_to")]
    pub in_reply_to: Option<InReplyTo>,
    #[serde(rename = "rel_type")]
    pub rel_type: Option<String>,
    #[serde(rename = "event_id")]
    pub event_id: Option<String>,
}

/// In-reply-to information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InReplyTo {
    pub event_id: String,
}

/// Mentions in messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mentions {
    pub user_ids: Vec<String>,
    pub room: bool,
}

/// Thread information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadInfo {
    pub event_id: String,
    pub is_falling_back: Option<bool>,
    pub is_editing: Option<bool>,
}

/// Enhanced room creation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedRoomCreateRequest {
    pub name: Option<String>,
    pub topic: Option<String>,
    pub preset: Option<RoomPreset>,
    pub room_version: Option<String>,
    pub is_direct: Option<bool>,
    pub invite: Option<Vec<String>>,
    pub initial_state: Option<Vec<InitialStateEvent>>,
}

/// Room presets
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RoomPreset {
    #[serde(rename = "private_chat")]
    PrivateChat,
    #[serde(rename = "trusted_private_chat")]
    TrustedPrivateChat,
    #[serde(rename = "public_chat")]
    PublicChat,
}

/// Initial state events for room creation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitialStateEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub state_key: String,
    pub content: serde_json::Value,
}

/// Enhanced room member information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedRoomMember {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub membership: MembershipState,
    pub power_level: i32,
    pub is_direct: bool,
    pub third_party_invite: Option<ThirdPartyInvite>,
}

/// Third party invite information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThirdPartyInvite {
    pub display_name: String,
    pub signed: SignedInvite,
}

/// Signed invite information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedInvite {
    pub mxid: String,
    pub token: String,
    pub signatures: HashMap<String, HashMap<String, String>>,
}

/// Enhanced room summary for listings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedRoomSummary {
    pub room_id: String,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub avatar_url: Option<String>,
    pub member_count: u32,
    pub is_public: bool,
    pub last_activity: u64,
    pub unread_count: u32,
    pub highlight_count: u32,
    pub notification_count: u32,
}

/// Enhanced user profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedUserProfile {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_online: bool,
    pub last_seen: Option<u64>,
    pub presence: PresenceState,
    pub status_msg: Option<String>,
}

/// Presence states
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PresenceState {
    #[serde(rename = "online")]
    Online,
    #[serde(rename = "offline")]
    Offline,
    #[serde(rename = "unavailable")]
    Unavailable,
}

/// Helper functions for enhanced types
impl EnhancedRoomInfo {
    pub fn new(room_id: String) -> Self {
        Self {
            room_id,
            name: None,
            topic: None,
            avatar_url: None,
            member_count: 0,
            is_public: false,
            join_rule: JoinRule::Invite,
            power_levels: PowerLevels::default(),
            creation_time: 0,
            last_activity: 0,
        }
    }
    
    pub fn is_admin(&self, user_id: &str) -> bool {
        self.power_levels.users.get(user_id)
            .map(|level| *level >= 100)
            .unwrap_or(false)
    }
    
    pub fn can_send_message(&self, user_id: &str) -> bool {
        let user_level = self.power_levels.users.get(user_id).unwrap_or(&0);
        *user_level >= self.power_levels.events_default
    }
}

impl EnhancedUserInfo {
    pub fn new(user_id: String) -> Self {
        Self {
            user_id,
            display_name: None,
            avatar_url: None,
            is_online: false,
            last_seen: None,
            power_level: 0,
            membership_state: MembershipState::Leave,
        }
    }
    
    pub fn is_member(&self) -> bool {
        self.membership_state == MembershipState::Join
    }
    
    pub fn is_moderator(&self) -> bool {
        self.power_level >= 50
    }
}

impl EnhancedMessageContent {
    pub fn new_text(body: String) -> Self {
        Self {
            msgtype: MessageType::Text,
            body,
            formatted_body: None,
            format: None,
            relates_to: None,
            mentions: None,
            thread: None,
        }
    }
    
    pub fn new_notice(body: String) -> Self {
        Self {
            msgtype: MessageType::Notice,
            body,
            formatted_body: None,
            format: None,
            relates_to: None,
            mentions: None,
            thread: None,
        }
    }
    
    pub fn is_reply(&self) -> bool {
        self.relates_to.as_ref()
            .and_then(|r| r.in_reply_to.as_ref())
            .is_some()
    }
    
    pub fn is_thread(&self) -> bool {
        self.thread.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_enhanced_room_info_creation() {
        let room = EnhancedRoomInfo::new("!test:example.com".to_string());
        assert_eq!(room.room_id, "!test:example.com");
        assert_eq!(room.member_count, 0);
        assert_eq!(room.join_rule, JoinRule::Invite);
    }
    
    #[test]
    fn test_enhanced_user_info_creation() {
        let user = EnhancedUserInfo::new("@user:example.com".to_string());
        assert_eq!(user.user_id, "@user:example.com");
        assert!(!user.is_online);
        assert_eq!(user.membership_state, MembershipState::Leave);
    }
    
    #[test]
    fn test_enhanced_message_content_creation() {
        let message = EnhancedMessageContent::new_text("Hello world".to_string());
        assert_eq!(message.msgtype, MessageType::Text);
        assert_eq!(message.body, "Hello world");
        assert!(!message.is_reply());
        assert!(!message.is_thread());
    }
    
    #[test]
    fn test_room_admin_check() {
        let mut room = EnhancedRoomInfo::new("!test:example.com".to_string());
        room.power_levels.users.insert("@admin:example.com".to_string(), 100);
        room.power_levels.users.insert("@user:example.com".to_string(), 0);
        
        assert!(room.is_admin("@admin:example.com"));
        assert!(!room.is_admin("@user:example.com"));
        assert!(!room.is_admin("@nonexistent:example.com"));
    }
    
    #[test]
    fn test_room_message_permission() {
        let mut room = EnhancedRoomInfo::new("!test:example.com".to_string());
        room.power_levels.events_default = 0;
        room.power_levels.users.insert("@user:example.com".to_string(), 0);
        room.power_levels.users.insert("@muted:example.com".to_string(), -1);
        
        assert!(room.can_send_message("@user:example.com"));
        assert!(!room.can_send_message("@muted:example.com"));
    }
    
    #[test]
    fn test_user_membership_check() {
        let mut user = EnhancedUserInfo::new("@user:example.com".to_string());
        user.membership_state = MembershipState::Join;
        assert!(user.is_member());
        
        user.membership_state = MembershipState::Leave;
        assert!(!user.is_member());
    }
    
    #[test]
    fn test_user_moderator_check() {
        let mut user = EnhancedUserInfo::new("@user:example.com".to_string());
        user.power_level = 50;
        assert!(user.is_moderator());
        
        user.power_level = 49;
        assert!(!user.is_moderator());
    }
    
    #[test]
    fn test_message_reply_check() {
        let mut message = EnhancedMessageContent::new_text("Reply".to_string());
        message.relates_to = Some(RelatesTo {
            in_reply_to: Some(InReplyTo {
                event_id: "$event123".to_string(),
            }),
            rel_type: None,
            event_id: None,
        });
        
        assert!(message.is_reply());
    }
    
    #[test]
    fn test_message_thread_check() {
        let mut message = EnhancedMessageContent::new_text("Thread message".to_string());
        message.thread = Some(ThreadInfo {
            event_id: "$thread123".to_string(),
            is_falling_back: None,
            is_editing: None,
        });
        
        assert!(message.is_thread());
    }
    
    #[test]
    fn test_power_levels_default() {
        let power_levels = PowerLevels::default();
        assert_eq!(power_levels.events_default, 0);
        assert_eq!(power_levels.state_default, 50);
        assert_eq!(power_levels.ban, 50);
        assert_eq!(power_levels.kick, 50);
        assert_eq!(power_levels.redact, 50);
        assert_eq!(power_levels.invite, 0);
    }
}
