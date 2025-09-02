# Enhanced Best Practices for Continuwuity

This document outlines best practices for using our enhanced error handling and type definitions in Continuwuity applications.

## Error Handling Best Practices

### 1. Use Enhanced Error Types

Always prefer enhanced error types over generic errors for better user experience:

```rust
// ✅ Good
let error = EnhancedError::RoomNotFoundError { 
    room_id: "!room123:example.com".to_string() 
};

// ❌ Avoid
let error = Error::BadRequest(ErrorKind::NotFound, "Room not found");
```

### 2. Provide Contextual Information

Include relevant context in error messages:

```rust
// ✅ Good
let error = EnhancedError::InsufficientPermissionsError {
    user_id: "@user:example.com".to_string(),
    message: "Admin role required for this operation".to_string()
};
```

### 3. Sanitize Error Messages for Logging

Always sanitize error messages before logging to prevent data leaks:

```rust
// ✅ Good
tracing::error!("Database operation failed: {}", error.sanitized_message());
```

## Type Definition Best Practices

### 1. Use Enhanced Types for Better Structure

Prefer enhanced types over basic types for complex operations:

```rust
// ✅ Good
let room_info = EnhancedRoomInfo {
    room_id: room_id!("!room123:example.com").to_owned(),
    name: Some("Test Room".to_string()),
    // ... other fields
};
```

### 2. Leverage Built-in Methods

Use the built-in methods provided by enhanced types:

```rust
// ✅ Good
if room_info.is_admin(&user_id) {
    // Admin operations
}

if room_info.can_send_message(&user_id) {
    // Send message
}
```

## API Design Best Practices

### 1. Consistent Error Responses

Ensure all API endpoints return consistent error responses:

```rust
async fn api_endpoint() -> Result<Json<Response>, (StatusCode, Json<serde_json::Value>)> {
    match operation() {
        Ok(response) => Ok(Json(response)),
        Err(error) => {
            let enhanced_error = match error {
                SomeError::NotFound => EnhancedError::RoomNotFoundError { 
                    room_id: "unknown".to_string() 
                },
                _ => EnhancedError::InternalServerError { 
                    message: "Unexpected error".to_string() 
                }
            };
            
            Err((
                StatusCode::from_u16(enhanced_error.http_status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                Json(serde_json::json!({
                    "errcode": enhanced_error.matrix_error_code(),
                    "error": enhanced_error.user_message()
                }))
            ))
        }
    }
}
```

## Testing Best Practices

### 1. Test Error Scenarios

Always test error scenarios to ensure proper error handling:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_room_not_found_error() {
        let error = EnhancedError::RoomNotFoundError { 
            room_id: "!test:matrix.org".to_string() 
        };
        
        assert_eq!(error.matrix_error_code(), "M_NOT_FOUND");
        assert_eq!(error.http_status_code(), 404);
        assert!(error.user_message().contains("could not be found"));
    }
}
```

## Security Best Practices

### 1. Validate Permissions

Always validate permissions before performing operations:

```rust
fn delete_room(room_info: &EnhancedRoomInfo, user_id: &OwnedUserId) -> Result<(), EnhancedError> {
    if !room_info.is_admin(user_id) {
        return Err(EnhancedError::InsufficientPermissionsError {
            user_id: user_id.to_string(),
            message: "Admin role required to delete rooms".to_string()
        });
    }
    
    // Proceed with deletion
    Ok(())
}
```

Following these best practices will help you build more robust, secure, and maintainable Continuwuity applications.