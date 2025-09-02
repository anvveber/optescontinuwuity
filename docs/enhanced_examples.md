# Enhanced Examples for Continuwuity

This document provides practical examples of using our enhanced error handling and type definitions in Continuwuity applications.

## Basic Error Handling Examples

### Authentication Error Handling

```rust
use conduwuit_core::error::*;

async fn authenticate_user(token: &str) -> Result<UserId, EnhancedError> {
    if token.is_empty() {
        return Err(EnhancedError::AuthenticationError {
            message: "Token is required".to_string()
        });
    }
    
    if token == "invalid" {
        return Err(EnhancedError::AuthenticationError {
            message: "Invalid token provided".to_string()
        });
    }
    
    Ok(user_id!("@user:example.com"))
}
```

### Room Management Examples

```rust
use conduwuit_core::types_enhanced::*;
use conduwuit_core::error::*;

async fn get_room_info(room_id_str: &str) -> Result<EnhancedRoomInfo, EnhancedError> {
    let room_id = room_id_str.parse::<OwnedRoomId>()
        .map_err(|_| EnhancedError::ValidationError {
            field: "room_id".to_string(),
            message: "Invalid room ID format".to_string()
        })?;
    
    if room_id_str == "!nonexistent:example.com" {
        return Err(EnhancedError::RoomNotFoundError {
            room_id: room_id_str.to_string()
        });
    }
    
    Ok(EnhancedRoomInfo {
        room_id: room_id.clone(),
        name: Some("Example Room".to_string()),
        topic: Some("A room for examples".to_string()),
        creator: user_id!("@creator:example.com").to_owned(),
        member_count: 5,
        join_rule: JoinRule::Public,
        power_levels: create_default_power_levels(),
        creation_content: RoomCreateEventContent::new(user_id!("@creator:example.com").to_owned()),
        is_direct: false,
        notification_count: 0,
    })
}
```

## API Endpoint Examples

### Room Information Endpoint

```rust
use axum::extract::Path;
use axum::response::Json;
use axum::http::StatusCode;
use conduwuit_core::error::*;
use conduwuit_core::types_enhanced::*;

async fn get_room_endpoint(
    Path(room_id): Path<String>
) -> Result<Json<EnhancedRoomInfo>, (StatusCode, Json<serde_json::Value>)> {
    match get_room_info(&room_id).await {
        Ok(room_info) => Ok(Json(room_info)),
        Err(error) => {
            let status_code = StatusCode::from_u16(error.http_status_code())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            
            let error_response = serde_json::json!({
                "errcode": error.matrix_error_code(),
                "error": error.user_message()
            });
            
            Err((status_code, Json(error_response)))
        }
    }
}
```

## Testing Examples

### Unit Test Examples

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_authenticate_user_success() {
        let result = authenticate_user("valid_token").await;
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_authenticate_user_invalid_token() {
        let result = authenticate_user("invalid").await;
        assert!(matches!(result, Err(EnhancedError::AuthenticationError { .. })));
    }
    
    #[test]
    fn test_enhanced_room_info_permissions() {
        let room_info = create_test_room_info();
        let admin_id = user_id!("@admin:example.com");
        let user_id = user_id!("@user:example.com");
        
        assert!(room_info.is_admin(&admin_id));
        assert!(!room_info.is_admin(&user_id));
        assert!(room_info.can_send_message(&user_id));
    }
}
```

These examples demonstrate how to effectively use our enhanced error handling and type definitions in real Continuwuity applications.