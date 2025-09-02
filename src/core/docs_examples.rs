//! Comprehensive Examples for Continuwuity
//! 
//! This module provides practical examples for common Matrix operations
//! using our enhanced types and error handling.

use crate::types::*;
use crate::error::*;

/// Example: Creating a new room with enhanced types
/// 
/// This example shows how to create a new Matrix room using our enhanced
/// room creation request structure.
/// 
/// # Example
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
/// 
/// // The room request is now ready to be sent to the Matrix server
/// ```
pub fn create_room_example() -> EnhancedRoomCreateRequest {
    EnhancedRoomCreateRequest {
        name: Some("Example Room".to_string()),
        topic: Some("This is an example room created with enhanced types".to_string()),
        preset: Some(RoomPreset::PublicChat),
        room_version: Some("10".to_string()),
        is_direct: Some(false),
        invite: Some(vec!["@alice:example.com".to_string(), "@bob:example.com".to_string()]),
        initial_state: None,
    }
}

/// Example: Creating a text message with enhanced content
/// 
/// This example shows how to create a Matrix message using our enhanced
/// message content structure.
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let message = EnhancedMessageContent::new_text("Hello, Matrix!".to_string());
/// 
/// // The message is ready to be sent to a room
/// ```
pub fn create_message_example() -> EnhancedMessageContent {
    EnhancedMessageContent::new_text("Hello from Continuwuity!".to_string())
}

/// Example: Creating a reply message
/// 
/// This example shows how to create a reply to another message.
/// 
/// # Example
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
/// // The reply is now properly linked to the original message
/// ```
pub fn create_reply_example() -> EnhancedMessageContent {
    let mut reply = EnhancedMessageContent::new_text("This is a reply to your message".to_string());
    reply.relates_to = Some(RelatesTo {
        in_reply_to: Some(InReplyTo {
            event_id: "$example_event_id".to_string(),
        }),
        rel_type: None,
        event_id: None,
    });
    reply
}

/// Example: Creating a thread message
/// 
/// This example shows how to create a message in a thread.
/// 
/// # Example
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
/// // The message is now part of a thread
/// ```
pub fn create_thread_message_example() -> EnhancedMessageContent {
    let mut thread_message = EnhancedMessageContent::new_text("This is a thread message".to_string());
    thread_message.thread = Some(ThreadInfo {
        event_id: "$thread_root_event_id".to_string(),
        is_falling_back: None,
        is_editing: None,
    });
    thread_message
}

/// Example: Setting up room power levels
/// 
/// This example shows how to configure power levels for a room.
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// use std::collections::HashMap;
/// 
/// let mut power_levels = PowerLevels::default();
/// power_levels.users.insert("@admin:example.com".to_string(), 100);
/// power_levels.users.insert("@moderator:example.com".to_string(), 50);
/// power_levels.events_default = 0; // Anyone can send messages
/// power_levels.state_default = 50; // Only moderators can change state
/// 
/// // Power levels are now configured
/// ```
pub fn setup_power_levels_example() -> PowerLevels {
    let mut power_levels = PowerLevels::default();
    power_levels.users.insert("@admin:example.com".to_string(), 100);
    power_levels.users.insert("@moderator:example.com".to_string(), 50);
    power_levels.users.insert("@user:example.com".to_string(), 0);
    power_levels.events_default = 0; // Anyone can send messages
    power_levels.state_default = 50; // Only moderators can change state
    power_levels.ban = 50; // Moderators can ban users
    power_levels.kick = 50; // Moderators can kick users
    power_levels
}

/// Example: Creating a user profile
/// 
/// This example shows how to create a user profile with enhanced information.
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// let user_profile = EnhancedUserProfile {
///     user_id: "@alice:example.com".to_string(),
///     display_name: Some("Alice".to_string()),
///     avatar_url: Some("mxc://example.com/avatar123".to_string()),
///     is_online: true,
///     last_seen: Some(1640995200), // Unix timestamp
///     presence: PresenceState::Online,
///     status_msg: Some("Working on Matrix stuff".to_string()),
/// };
/// 
/// // User profile is now complete
/// ```
pub fn create_user_profile_example() -> EnhancedUserProfile {
    EnhancedUserProfile {
        user_id: "@alice:example.com".to_string(),
        display_name: Some("Alice".to_string()),
        avatar_url: Some("mxc://example.com/avatar123".to_string()),
        is_online: true,
        last_seen: Some(1640995200), // Unix timestamp
        presence: PresenceState::Online,
        status_msg: Some("Working on Matrix stuff".to_string()),
    }
}

/// Example: Error handling with enhanced errors
/// 
/// This example shows how to use our enhanced error handling for better
/// user experience and debugging.
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::error::*;
/// 
/// // Create a user-friendly error
/// let error = EnhancedError::room_not_found("!nonexistent:example.com");
/// 
/// // Get user-friendly message
/// let user_message = error.user_message();
/// println!("User sees: {}", user_message);
/// 
/// // Get sanitized message for logging
/// let log_message = error.sanitized_message();
/// println!("Log shows: {}", log_message);
/// 
/// // Get Matrix error code
/// let matrix_code = error.matrix_error_code();
/// println!("Matrix error code: {}", matrix_code);
/// ```
pub fn error_handling_example() -> EnhancedError {
    let error = EnhancedError::room_not_found("!example:example.com");
    
    // Demonstrate different error message types
    let _user_message = error.user_message();
    let _log_message = error.sanitized_message();
    let _matrix_code = error.matrix_error_code();
    
    error
}

/// Example: Room member management
/// 
/// This example shows how to manage room members using enhanced types.
/// 
/// # Example
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
/// 
/// // Member information is now available
/// ```
pub fn room_member_example() -> EnhancedRoomMember {
    EnhancedRoomMember {
        user_id: "@alice:example.com".to_string(),
        display_name: Some("Alice".to_string()),
        avatar_url: Some("mxc://example.com/avatar123".to_string()),
        membership: MembershipState::Join,
        power_level: 50,
        is_direct: false,
        third_party_invite: None,
    }
}

/// Example: Room summary for listings
/// 
/// This example shows how to create a room summary for display in room lists.
/// 
/// # Example
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
/// 
/// // Room summary is ready for display
/// ```
pub fn room_summary_example() -> EnhancedRoomSummary {
    EnhancedRoomSummary {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_create_room_example() {
        let room_request = create_room_example();
        assert_eq!(room_request.name, Some("Example Room".to_string()));
        assert_eq!(room_request.preset, Some(RoomPreset::PublicChat));
        assert!(room_request.invite.is_some());
    }
    
    #[test]
    fn test_create_message_example() {
        let message = create_message_example();
        assert_eq!(message.msgtype, MessageType::Text);
        assert_eq!(message.body, "Hello from Continuwuity!");
    }
    
    #[test]
    fn test_create_reply_example() {
        let reply = create_reply_example();
        assert!(reply.is_reply());
        assert!(reply.relates_to.is_some());
    }
    
    #[test]
    fn test_create_thread_message_example() {
        let thread_message = create_thread_message_example();
        assert!(thread_message.is_thread());
        assert!(thread_message.thread.is_some());
    }
    
    #[test]
    fn test_setup_power_levels_example() {
        let power_levels = setup_power_levels_example();
        assert_eq!(power_levels.users.len(), 3);
        assert_eq!(power_levels.events_default, 0);
        assert_eq!(power_levels.state_default, 50);
    }
    
    #[test]
    fn test_create_user_profile_example() {
        let profile = create_user_profile_example();
        assert_eq!(profile.user_id, "@alice:example.com");
        assert_eq!(profile.presence, PresenceState::Online);
        assert!(profile.is_online);
    }
    
    #[test]
    fn test_error_handling_example() {
        let error = error_handling_example();
        assert_eq!(error.matrix_error_code(), "M_NOT_FOUND");
        assert_eq!(error.http_status_code(), 404);
    }
    
    #[test]
    fn test_room_member_example() {
        let member = room_member_example();
        assert_eq!(member.user_id, "@alice:example.com");
        assert_eq!(member.membership, MembershipState::Join);
        assert_eq!(member.power_level, 50);
    }
    
    #[test]
    fn test_room_summary_example() {
        let summary = room_summary_example();
        assert_eq!(summary.room_id, "!room123:example.com");
        assert_eq!(summary.member_count, 42);
        assert!(summary.is_public);
    }
}
