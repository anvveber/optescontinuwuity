//! API Guide for Continuwuity Enhanced Types
//! 
//! This module provides a comprehensive guide for using our enhanced types
//! and error handling in Continuwuity applications.

use crate::types::*;
use crate::error::*;

/// API Guide: Enhanced Error Handling
/// 
/// Our enhanced error system provides better user experience and debugging capabilities.
/// 
/// # Key Features
/// 
/// - **User-friendly messages**: Clear, actionable error messages for users
/// - **Sanitized logging**: Safe error messages for logs without sensitive data
/// - **Matrix compliance**: Proper Matrix error codes and HTTP status codes
/// - **Contextual information**: Rich error context for better debugging
/// 
/// # Usage Examples
/// 
/// ## Creating Errors
/// 
/// ```rust
/// use conduwuit_core::error::*;
/// 
/// // Authentication errors
/// let auth_error = EnhancedError::auth_failed("Invalid token");
/// 
/// // Room errors
/// let room_error = EnhancedError::room_not_found("!room123:example.com");
/// 
/// // User errors
/// let user_error = EnhancedError::user_not_found("@user:example.com");
/// 
/// // Permission errors
/// let perm_error = EnhancedError::insufficient_permissions("Admin role required");
/// 
/// // Federation errors
/// let fed_error = EnhancedError::federation_error("matrix.org", "Connection timeout");
/// 
/// // Configuration errors
/// let config_error = EnhancedError::config_error("database_url", "Invalid format");
/// 
/// // Database errors
/// let db_error = EnhancedError::database_error("user_lookup", "Connection lost");
/// 
/// // Network errors
/// let net_error = EnhancedError::network_error("federation", "DNS resolution failed");
/// 
/// // Validation errors
/// let val_error = EnhancedError::validation_error("room_name", "Too long");
/// 
/// // Rate limit errors
/// let rate_error = EnhancedError::rate_limit_exceeded("room_creation", 60);
/// ```
/// 
/// ## Using Error Information
/// 
/// ```rust
/// use conduwuit_core::error::*;
/// 
/// let error = EnhancedError::room_not_found("!room123:example.com");
/// 
/// // Get user-friendly message
/// let user_msg = error.user_message();
/// // "The room '!room123:example.com' could not be found. It may have been deleted or you may not have access."
/// 
/// // Get sanitized message for logging
/// let log_msg = error.sanitized_message();
/// // "Room not found: !room123:example.com"
/// 
/// // Get Matrix error code
/// let matrix_code = error.matrix_error_code();
/// // "M_NOT_FOUND"
/// 
/// // Get HTTP status code
/// let http_code = error.http_status_code();
/// // 404
/// ```
pub fn enhanced_error_guide() {
    // This function demonstrates the enhanced error handling API
    let _auth_error = EnhancedError::auth_failed("Invalid token");
    let _room_error = EnhancedError::room_not_found("!room123:example.com");
    let _user_error = EnhancedError::user_not_found("@user:example.com");
}

/// API Guide: Enhanced Room Types
/// 
/// Our enhanced room types provide better organization and functionality for room operations.
/// 
/// # Key Features
/// 
/// - **Rich room information**: Comprehensive room data with metadata
/// - **Permission checking**: Built-in methods for checking user permissions
/// - **Power level management**: Easy power level configuration and checking
/// - **State management**: Proper room state handling
/// 
/// # Usage Examples
/// 
/// ## Creating Room Information
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let room = EnhancedRoomInfo::new("!room123:example.com".to_string());
/// 
/// // Set room properties
/// let mut room = EnhancedRoomInfo::new("!room123:example.com".to_string());
/// room.name = Some("My Awesome Room".to_string());
/// room.topic = Some("A room for awesome discussions".to_string());
/// room.is_public = true;
/// room.join_rule = JoinRule::Public;
/// ```
/// 
/// ## Checking Permissions
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let room = EnhancedRoomInfo::new("!room123:example.com".to_string());
/// 
/// // Check if user is admin
/// let is_admin = room.is_admin("@admin:example.com");
/// 
/// // Check if user can send messages
/// let can_send = room.can_send_message("@user:example.com");
/// ```
/// 
/// ## Power Level Management
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// use std::collections::HashMap;
/// 
/// let mut power_levels = PowerLevels::default();
/// power_levels.users.insert("@admin:example.com".to_string(), 100);
/// power_levels.users.insert("@moderator:example.com".to_string(), 50);
/// power_levels.events_default = 0;
/// power_levels.state_default = 50;
/// ```
pub fn enhanced_room_guide() {
    // This function demonstrates the enhanced room types API
    let room = EnhancedRoomInfo::new("!room123:example.com".to_string());
    let _is_admin = room.is_admin("@admin:example.com");
    let _can_send = room.can_send_message("@user:example.com");
}

/// API Guide: Enhanced User Types
/// 
/// Our enhanced user types provide better user information and management.
/// 
/// # Key Features
/// 
/// - **Rich user information**: Comprehensive user data with presence
/// - **Membership checking**: Built-in methods for checking user membership
/// - **Permission checking**: Easy permission level checking
/// - **Profile management**: Complete user profile information
/// 
/// # Usage Examples
/// 
/// ## Creating User Information
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let user = EnhancedUserInfo::new("@user:example.com".to_string());
/// 
/// // Set user properties
/// let mut user = EnhancedUserInfo::new("@user:example.com".to_string());
/// user.display_name = Some("Alice".to_string());
/// user.avatar_url = Some("mxc://example.com/avatar123".to_string());
/// user.is_online = true;
/// user.power_level = 50;
/// user.membership_state = MembershipState::Join;
/// ```
/// 
/// ## Checking User Status
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let user = EnhancedUserInfo::new("@user:example.com".to_string());
/// 
/// // Check if user is a member
/// let is_member = user.is_member();
/// 
/// // Check if user is a moderator
/// let is_moderator = user.is_moderator();
/// ```
/// 
/// ## User Profiles
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let profile = EnhancedUserProfile {
///     user_id: "@alice:example.com".to_string(),
///     display_name: Some("Alice".to_string()),
///     avatar_url: Some("mxc://example.com/avatar123".to_string()),
///     is_online: true,
///     last_seen: Some(1640995200),
///     presence: PresenceState::Online,
///     status_msg: Some("Working on Matrix stuff".to_string()),
/// };
/// ```
pub fn enhanced_user_guide() {
    // This function demonstrates the enhanced user types API
    let user = EnhancedUserInfo::new("@user:example.com".to_string());
    let _is_member = user.is_member();
    let _is_moderator = user.is_moderator();
}

/// API Guide: Enhanced Message Types
/// 
/// Our enhanced message types provide better message handling and processing.
/// 
/// # Key Features
/// 
/// - **Rich message content**: Comprehensive message data with metadata
/// - **Reply handling**: Built-in support for message replies
/// - **Thread support**: Easy thread message handling
/// - **Mention support**: Built-in mention handling
/// 
/// # Usage Examples
/// 
/// ## Creating Messages
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// // Create a simple text message
/// let message = EnhancedMessageContent::new_text("Hello, world!".to_string());
/// 
/// // Create a notice message
/// let notice = EnhancedMessageContent::new_notice("System message".to_string());
/// ```
/// 
/// ## Creating Replies
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let mut reply = EnhancedMessageContent::new_text("This is a reply".to_string());
/// reply.relates_to = Some(RelatesTo {
///     in_reply_to: Some(InReplyTo {
///         event_id: "$original_event_id".to_string(),
///     }),
///     rel_type: None,
///     event_id: None,
/// });
/// 
/// // Check if message is a reply
/// let is_reply = reply.is_reply();
/// ```
/// 
/// ## Creating Thread Messages
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let mut thread_message = EnhancedMessageContent::new_text("Thread message".to_string());
/// thread_message.thread = Some(ThreadInfo {
///     event_id: "$thread_root_event_id".to_string(),
///     is_falling_back: None,
///     is_editing: None,
/// });
/// 
/// // Check if message is in a thread
/// let is_thread = thread_message.is_thread();
/// ```
/// 
/// ## Adding Mentions
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let mut message = EnhancedMessageContent::new_text("Hello @alice:example.com!".to_string());
/// message.mentions = Some(Mentions {
///     user_ids: vec!["@alice:example.com".to_string()],
///     room: false,
/// });
/// ```
pub fn enhanced_message_guide() {
    // This function demonstrates the enhanced message types API
    let message = EnhancedMessageContent::new_text("Hello, world!".to_string());
    let _is_reply = message.is_reply();
    let _is_thread = message.is_thread();
}

/// API Guide: Room Creation and Management
/// 
/// Our enhanced types provide better room creation and management capabilities.
/// 
/// # Key Features
/// 
/// - **Structured room creation**: Well-defined room creation requests
/// - **Member management**: Easy room member handling
/// - **Room summaries**: Rich room information for listings
/// - **State management**: Proper room state handling
/// 
/// # Usage Examples
/// 
/// ## Creating Rooms
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let room_request = EnhancedRoomCreateRequest {
///     name: Some("My Awesome Room".to_string()),
///     topic: Some("A room for awesome discussions".to_string()),
///     preset: Some(RoomPreset::PublicChat),
///     room_version: Some("10".to_string()),
///     is_direct: Some(false),
///     invite: Some(vec!["@alice:example.com".to_string()]),
///     initial_state: None,
/// };
/// ```
/// 
/// ## Managing Room Members
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let member = EnhancedRoomMember {
///     user_id: "@alice:example.com".to_string(),
///     display_name: Some("Alice".to_string()),
///     avatar_url: Some("mxc://example.com/avatar123".to_string()),
///     membership: MembershipState::Join,
///     power_level: 50,
///     is_direct: false,
///     third_party_invite: None,
/// };
/// ```
/// 
/// ## Room Summaries
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let room_summary = EnhancedRoomSummary {
///     room_id: "!room123:example.com".to_string(),
///     name: Some("General Discussion".to_string()),
///     topic: Some("A place for general chat".to_string()),
///     avatar_url: Some("mxc://example.com/room_avatar".to_string()),
///     member_count: 42,
///     is_public: true,
///     last_activity: 1640995200,
///     unread_count: 5,
///     highlight_count: 2,
///     notification_count: 7,
/// };
/// ```
pub fn room_management_guide() {
    // This function demonstrates the room management API
    let _room_request = EnhancedRoomCreateRequest {
        name: Some("Example Room".to_string()),
        topic: Some("Example topic".to_string()),
        preset: Some(RoomPreset::PublicChat),
        room_version: Some("10".to_string()),
        is_direct: Some(false),
        invite: None,
        initial_state: None,
    };
    
    let _room_summary = EnhancedRoomSummary {
        room_id: "!room123:example.com".to_string(),
        name: Some("General Discussion".to_string()),
        topic: Some("A place for general chat".to_string()),
        avatar_url: Some("mxc://example.com/room_avatar".to_string()),
        member_count: 42,
        is_public: true,
        last_activity: 1640995200,
        unread_count: 5,
        highlight_count: 2,
        notification_count: 7,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_enhanced_error_guide() {
        enhanced_error_guide();
    }
    
    #[test]
    fn test_enhanced_room_guide() {
        enhanced_room_guide();
    }
    
    #[test]
    fn test_enhanced_user_guide() {
        enhanced_user_guide();
    }
    
    #[test]
    fn test_enhanced_message_guide() {
        enhanced_message_guide();
    }
    
    #[test]
    fn test_room_management_guide() {
        room_management_guide();
    }
}
