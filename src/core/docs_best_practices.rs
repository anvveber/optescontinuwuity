//! Best Practices for Continuwuity Development
//! 
//! This module provides best practices and guidelines for developing
//! with Continuwuity and our enhanced types.

use crate::types::*;
use crate::error::*;

/// Best Practice: Error Handling
/// 
/// Always use our enhanced error types for better user experience.
/// 
/// # Best Practices
/// 
/// 1. **Use Enhanced Errors**: Always use `EnhancedError` for user-facing errors
/// 2. **Provide Context**: Include relevant context in error messages
/// 3. **Sanitize Logs**: Use `sanitized_message()` for logging to avoid sensitive data
/// 4. **Return Proper Codes**: Ensure Matrix error codes are correct
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::error::*;
/// 
/// fn validate_room_id(room_id: &str) -> Result<(), EnhancedError> {
///     if room_id.is_empty() {
///         return Err(EnhancedError::validation_error("room_id", "Room ID cannot be empty"));
///     }
///     
///     if !room_id.starts_with('!') {
///         return Err(EnhancedError::validation_error("room_id", "Room ID must start with '!'"));
///     }
///     
///     Ok(())
/// }
/// ```
pub fn error_handling_best_practices() {
    // This function demonstrates error handling best practices
    let _example_error = EnhancedError::validation_error("room_id", "Room ID cannot be empty");
}

/// Best Practice: Room Management
/// 
/// Use enhanced types for room operations to ensure consistency.
/// 
/// # Best Practices
/// 
/// 1. **Use Enhanced Types**: Always use `EnhancedRoomInfo` for room data
/// 2. **Validate Permissions**: Check power levels before operations
/// 3. **Handle State Changes**: Properly manage room state transitions
/// 4. **Log Important Events**: Log room creation, member changes, etc.
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// fn can_user_send_message(room: &EnhancedRoomInfo, user_id: &str) -> bool {
///     room.can_send_message(user_id)
/// }
/// 
/// fn is_user_admin(room: &EnhancedRoomInfo, user_id: &str) -> bool {
///     room.is_admin(user_id)
/// }
/// ```
pub fn room_management_best_practices() {
    // This function demonstrates room management best practices
    let room = EnhancedRoomInfo::new("!example:example.com".to_string());
    let _can_send = room.can_send_message("@user:example.com");
    let _is_admin = room.is_admin("@admin:example.com");
}

/// Best Practice: Message Handling
/// 
/// Use enhanced message types for consistent message processing.
/// 
/// # Best Practices
/// 
/// 1. **Use Enhanced Types**: Always use `EnhancedMessageContent` for messages
/// 2. **Handle Replies**: Properly process reply relationships
/// 3. **Support Threads**: Handle thread messages correctly
/// 4. **Validate Content**: Check message content before processing
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// fn process_message(message: &EnhancedMessageContent) -> Result<(), EnhancedError> {
///     if message.body.is_empty() {
///         return Err(EnhancedError::validation_error("body", "Message body cannot be empty"));
///     }
///     
///     if message.is_reply() {
///         // Handle reply logic
///     }
///     
///     if message.is_thread() {
///         // Handle thread logic
///     }
///     
///     Ok(())
/// }
/// ```
pub fn message_handling_best_practices() {
    // This function demonstrates message handling best practices
    let message = EnhancedMessageContent::new_text("Hello, world!".to_string());
    let _is_reply = message.is_reply();
    let _is_thread = message.is_thread();
}

/// Best Practice: User Management
/// 
/// Use enhanced user types for consistent user operations.
/// 
/// # Best Practices
/// 
/// 1. **Use Enhanced Types**: Always use `EnhancedUserInfo` for user data
/// 2. **Check Membership**: Verify user membership before operations
/// 3. **Handle Permissions**: Check user permissions appropriately
/// 4. **Update Presence**: Keep user presence information current
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// 
/// fn can_user_access_room(user: &EnhancedUserInfo, room: &EnhancedRoomInfo) -> bool {
///     user.is_member() && room.can_send_message(&user.user_id)
/// }
/// 
/// fn is_user_moderator(user: &EnhancedUserInfo) -> bool {
///     user.is_moderator()
/// }
/// ```
pub fn user_management_best_practices() {
    // This function demonstrates user management best practices
    let user = EnhancedUserInfo::new("@user:example.com".to_string());
    let _is_member = user.is_member();
    let _is_moderator = user.is_moderator();
}

/// Best Practice: Power Level Management
/// 
/// Properly manage power levels for room permissions.
/// 
/// # Best Practices
/// 
/// 1. **Use Default Values**: Start with sensible defaults
/// 2. **Document Changes**: Log power level changes
/// 3. **Validate Levels**: Ensure power levels are within valid ranges
/// 4. **Handle Inheritance**: Properly handle power level inheritance
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::types::*;
/// use std::collections::HashMap;
/// 
/// fn set_user_power_level(power_levels: &mut PowerLevels, user_id: &str, level: i32) -> Result<(), EnhancedError> {
///     if level < -1 || level > 100 {
///         return Err(EnhancedError::validation_error("power_level", "Power level must be between -1 and 100"));
///     }
///     
///     power_levels.users.insert(user_id.to_string(), level);
///     Ok(())
/// }
/// ```
pub fn power_level_management_best_practices() {
    // This function demonstrates power level management best practices
    let mut power_levels = PowerLevels::default();
    power_levels.users.insert("@user:example.com".to_string(), 0);
}

/// Best Practice: Federation Handling
/// 
/// Properly handle federation operations with enhanced error types.
/// 
/// # Best Practices
/// 
/// 1. **Use Enhanced Errors**: Use `EnhancedError::federation_error` for federation issues
/// 2. **Handle Timeouts**: Implement proper timeout handling
/// 3. **Log Federation Events**: Log important federation operations
/// 4. **Validate Responses**: Validate federation responses before processing
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::error::*;
/// 
/// async fn send_federation_request(server: &str, data: &[u8]) -> Result<(), EnhancedError> {
///     // Simulate federation request
///     if server.is_empty() {
///         return Err(EnhancedError::federation_error("unknown", "Server name cannot be empty"));
///     }
///     
///     // Handle federation logic here
///     Ok(())
/// }
/// ```
pub fn federation_handling_best_practices() {
    // This function demonstrates federation handling best practices
    let _example_error = EnhancedError::federation_error("matrix.org", "Connection timeout");
}

/// Best Practice: Configuration Management
/// 
/// Use enhanced error types for configuration validation.
/// 
/// # Best Practices
/// 
/// 1. **Validate Early**: Validate configuration at startup
/// 2. **Use Enhanced Errors**: Use `EnhancedError::config_error` for config issues
/// 3. **Provide Context**: Include field names in error messages
/// 4. **Handle Defaults**: Provide sensible defaults where possible
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::error::*;
/// 
/// fn validate_config_field(field_name: &str, value: &str) -> Result<(), EnhancedError> {
///     if value.is_empty() {
///         return Err(EnhancedError::config_error(field_name, "Value cannot be empty"));
///     }
///     
///     Ok(())
/// }
/// ```
pub fn configuration_management_best_practices() {
    // This function demonstrates configuration management best practices
    let _example_error = EnhancedError::config_error("database_url", "Invalid format");
}

/// Best Practice: Database Operations
/// 
/// Use enhanced error types for database operations.
/// 
/// # Best Practices
/// 
/// 1. **Use Enhanced Errors**: Use `EnhancedError::database_error` for DB issues
/// 2. **Include Operation Context**: Specify the operation that failed
/// 3. **Handle Connections**: Properly handle database connection issues
/// 4. **Log Operations**: Log important database operations
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::error::*;
/// 
/// async fn save_user_data(user_id: &str, data: &[u8]) -> Result<(), EnhancedError> {
///     // Simulate database operation
///     if user_id.is_empty() {
///         return Err(EnhancedError::database_error("save_user_data", "User ID cannot be empty"));
///     }
///     
///     // Handle database logic here
///     Ok(())
/// }
/// ```
pub fn database_operations_best_practices() {
    // This function demonstrates database operations best practices
    let _example_error = EnhancedError::database_error("save_user_data", "Connection lost");
}

/// Best Practice: Network Operations
/// 
/// Use enhanced error types for network operations.
/// 
/// # Best Practices
/// 
/// 1. **Use Enhanced Errors**: Use `EnhancedError::network_error` for network issues
/// 2. **Specify Service**: Include the service that failed
/// 3. **Handle Timeouts**: Implement proper timeout handling
/// 4. **Retry Logic**: Implement appropriate retry logic
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::error::*;
/// 
/// async fn make_http_request(url: &str) -> Result<(), EnhancedError> {
///     if url.is_empty() {
///         return Err(EnhancedError::network_error("http_client", "URL cannot be empty"));
///     }
///     
///     // Handle HTTP request logic here
///     Ok(())
/// }
/// ```
pub fn network_operations_best_practices() {
    // This function demonstrates network operations best practices
    let _example_error = EnhancedError::network_error("http_client", "Connection timeout");
}

/// Best Practice: Rate Limiting
/// 
/// Use enhanced error types for rate limiting.
/// 
/// # Best Practices
/// 
/// 1. **Use Enhanced Errors**: Use `EnhancedError::rate_limit_exceeded` for rate limits
/// 2. **Include Resource**: Specify which resource was rate limited
/// 3. **Provide Retry Time**: Include when the user can retry
/// 4. **Log Rate Limits**: Log rate limit violations for monitoring
/// 
/// # Example
/// 
/// ```rust
/// use conduwuit_core::error::*;
/// 
/// fn check_rate_limit(resource: &str, user_id: &str) -> Result<(), EnhancedError> {
///     // Simulate rate limit check
///     if resource == "room_creation" {
///         return Err(EnhancedError::rate_limit_exceeded(resource, 60));
///     }
///     
///     Ok(())
/// }
/// ```
pub fn rate_limiting_best_practices() {
    // This function demonstrates rate limiting best practices
    let _example_error = EnhancedError::rate_limit_exceeded("room_creation", 60);
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_error_handling_best_practices() {
        error_handling_best_practices();
    }
    
    #[test]
    fn test_room_management_best_practices() {
        room_management_best_practices();
    }
    
    #[test]
    fn test_message_handling_best_practices() {
        message_handling_best_practices();
    }
    
    #[test]
    fn test_user_management_best_practices() {
        user_management_best_practices();
    }
    
    #[test]
    fn test_power_level_management_best_practices() {
        power_level_management_best_practices();
    }
    
    #[test]
    fn test_federation_handling_best_practices() {
        federation_handling_best_practices();
    }
    
    #[test]
    fn test_configuration_management_best_practices() {
        configuration_management_best_practices();
    }
    
    #[test]
    fn test_database_operations_best_practices() {
        database_operations_best_practices();
    }
    
    #[test]
    fn test_network_operations_best_practices() {
        network_operations_best_practices();
    }
    
    #[test]
    fn test_rate_limiting_best_practices() {
        rate_limiting_best_practices();
    }
}
