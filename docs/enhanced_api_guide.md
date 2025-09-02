# Enhanced API Guide for Continuwuity

This guide provides comprehensive documentation for using our enhanced types and error handling in Continuwuity applications.

## Enhanced Error Handling

Our enhanced error system provides better user experience and debugging capabilities.

### Key Features

- **User-friendly messages**: Clear, actionable error messages for users
- **Sanitized logging**: Safe error messages for logs without sensitive data
- **Matrix compliance**: Proper Matrix error codes and HTTP status codes
- **Contextual information**: Rich error context for better debugging

### Usage Examples

#### Creating Errors

```rust
use conduwuit_core::error::*;

// Authentication errors
let auth_error = EnhancedError::AuthenticationError { 
    message: "Invalid token".to_string() 
};

// Room errors
let room_error = EnhancedError::RoomNotFoundError { 
    room_id: "!room123:example.com".to_string() 
};

// User errors
let user_error = EnhancedError::UserNotFoundError { 
    user_id: "@user:example.com".to_string() 
};
```

#### Error Handling

```rust
use conduwuit_core::error::*;

fn handle_error(error: &EnhancedError) -> Result<(), String> {
    match error {
        EnhancedError::AuthenticationError { message } => {
            tracing::error!("Authentication failed: {}", message);
            Err("Please check your login credentials and try again.".to_string())
        },
        EnhancedError::RoomNotFoundError { room_id } => {
            tracing::warn!("Room not found: {}", room_id);
            Err(format!("The room '{}' could not be found.", room_id))
        },
        _ => {
            tracing::error!("Unexpected error: {}", error);
            Err("An unexpected error occurred. Please try again.".to_string())
        }
    }
}
```

## Enhanced Type Definitions

Our enhanced types provide cleaner, more focused structures for Matrix operations.

### Room Management

#### EnhancedRoomInfo

```rust
use conduwuit_core::types_enhanced::*;

// Create room info
let room_info = EnhancedRoomInfo {
    room_id: room_id!("!test:matrix.org").to_owned(),
    name: Some("Test Room".to_string()),
    topic: Some("A test room for development".to_string()),
    creator: user_id!("@creator:matrix.org").to_owned(),
    member_count: 5,
    join_rule: JoinRule::Public,
    power_levels: create_default_power_levels(),
    creation_content: RoomCreateEventContent::new(user_id!("@creator:matrix.org").to_owned()),
    is_direct: false,
    notification_count: 0,
};

// Check permissions
if room_info.is_admin(&user_id!("@admin:matrix.org")) {
    println!("User is an admin");
}
```

## Best Practices

1. **Always use enhanced error types** for better user experience
2. **Provide contextual information** in error messages
3. **Use proper Matrix error codes** for federation compatibility
4. **Sanitize error messages** before logging to prevent data leaks
5. **Leverage enhanced types** for cleaner, more maintainable code
6. **Test error scenarios** to ensure proper error handling

This enhanced API provides a more robust, user-friendly, and maintainable foundation for Continuwuity applications.