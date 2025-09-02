// Enhanced Error Handling for Continuwuity
// Contributed from our chat-system improvements
// Provides better user-friendly error messages and Matrix-specific error codes

use thiserror::Error;

/// Enhanced error types with better user messaging
#[derive(Error, Debug)]
pub enum EnhancedError {
    #[error("Authentication failed: {message}")]
    Authentication { message: String },
    
    #[error("Room not found: {room_id}")]
    RoomNotFound { room_id: String },
    
    #[error("User not found: {user_id}")]
    UserNotFound { user_id: String },
    
    #[error("Insufficient permissions: {reason}")]
    InsufficientPermissions { reason: String },
    
    #[error("Federation error with {server}: {message}")]
    FederationError { server: String, message: String },
    
    #[error("Configuration error: {field} - {message}")]
    ConfigurationError { field: String, message: String },
    
    #[error("Database error: {operation} failed - {message}")]
    DatabaseError { operation: String, message: String },
    
    #[error("Network error: {service} unavailable - {message}")]
    NetworkError { service: String, message: String },
    
    #[error("Validation error: {field} - {message}")]
    ValidationError { field: String, message: String },
    
    #[error("Rate limit exceeded: {resource} - retry after {seconds}s")]
    RateLimitExceeded { resource: String, seconds: u64 },
}

impl EnhancedError {
    /// Get the Matrix error code for this error
    pub fn matrix_error_code(&self) -> &'static str {
        match self {
            EnhancedError::Authentication { .. } => "M_UNAUTHORIZED",
            EnhancedError::RoomNotFound { .. } => "M_NOT_FOUND",
            EnhancedError::UserNotFound { .. } => "M_NOT_FOUND",
            EnhancedError::InsufficientPermissions { .. } => "M_FORBIDDEN",
            EnhancedError::FederationError { .. } => "M_FEDERATION_ERROR",
            EnhancedError::ConfigurationError { .. } => "M_UNKNOWN",
            EnhancedError::DatabaseError { .. } => "M_UNKNOWN",
            EnhancedError::NetworkError { .. } => "M_UNKNOWN",
            EnhancedError::ValidationError { .. } => "M_BAD_JSON",
            EnhancedError::RateLimitExceeded { .. } => "M_LIMIT_EXCEEDED",
        }
    }
    
    /// Get the HTTP status code for this error
    pub fn http_status_code(&self) -> u16 {
        match self {
            EnhancedError::Authentication { .. } => 401,
            EnhancedError::RoomNotFound { .. } | EnhancedError::UserNotFound { .. } => 404,
            EnhancedError::InsufficientPermissions { .. } => 403,
            EnhancedError::FederationError { .. } => 502,
            EnhancedError::ConfigurationError { .. } | EnhancedError::DatabaseError { .. } => 500,
            EnhancedError::NetworkError { .. } => 503,
            EnhancedError::ValidationError { .. } => 400,
            EnhancedError::RateLimitExceeded { .. } => 429,
        }
    }
    
    /// Get a user-friendly error message
    pub fn user_message(&self) -> String {
        match self {
            EnhancedError::Authentication { message } => {
                format!("Please check your login credentials. {}", message)
            },
            EnhancedError::RoomNotFound { room_id } => {
                format!("The room '{}' could not be found. It may have been deleted or you may not have access.", room_id)
            },
            EnhancedError::UserNotFound { user_id } => {
                format!("User '{}' was not found. Please check the username and try again.", user_id)
            },
            EnhancedError::InsufficientPermissions { reason } => {
                format!("You don't have permission to perform this action. {}", reason)
            },
            EnhancedError::FederationError { server, message } => {
                format!("Unable to communicate with server '{}'. Please try again later. {}", server, message)
            },
            EnhancedError::ConfigurationError { field, message } => {
                format!("Server configuration error in '{}': {}", field, message)
            },
            EnhancedError::DatabaseError { operation, message: _ } => {
                format!("Database operation '{}' failed. Please try again later.", operation)
            },
            EnhancedError::NetworkError { service, message } => {
                format!("Network service '{}' is currently unavailable. {}", service, message)
            },
            EnhancedError::ValidationError { field, message } => {
                format!("Invalid value for '{}': {}", field, message)
            },
            EnhancedError::RateLimitExceeded { resource, seconds } => {
                format!("Too many requests for '{}'. Please wait {} seconds before trying again.", resource, seconds)
            },
        }
    }
    
    /// Get a sanitized error message for logging (removes sensitive info)
    pub fn sanitized_message(&self) -> String {
        match self {
            EnhancedError::Authentication { .. } => "Authentication failed".to_string(),
            EnhancedError::RoomNotFound { room_id } => format!("Room not found: {}", room_id),
            EnhancedError::UserNotFound { user_id } => format!("User not found: {}", user_id),
            EnhancedError::InsufficientPermissions { .. } => "Insufficient permissions".to_string(),
            EnhancedError::FederationError { server, .. } => format!("Federation error with {}", server),
            EnhancedError::ConfigurationError { field, .. } => format!("Configuration error in {}", field),
            EnhancedError::DatabaseError { operation, .. } => format!("Database error in {}", operation),
            EnhancedError::NetworkError { service, .. } => format!("Network error in {}", service),
            EnhancedError::ValidationError { field, .. } => format!("Validation error in {}", field),
            EnhancedError::RateLimitExceeded { resource, seconds } => {
                format!("Rate limit exceeded for {} ({}s)", resource, seconds)
            },
        }
    }
}

/// Helper functions for creating common errors
impl EnhancedError {
    pub fn auth_failed(message: impl Into<String>) -> Self {
        Self::Authentication { message: message.into() }
    }
    
    pub fn room_not_found(room_id: impl Into<String>) -> Self {
        Self::RoomNotFound { room_id: room_id.into() }
    }
    
    pub fn user_not_found(user_id: impl Into<String>) -> Self {
        Self::UserNotFound { user_id: user_id.into() }
    }
    
    pub fn insufficient_permissions(reason: impl Into<String>) -> Self {
        Self::InsufficientPermissions { reason: reason.into() }
    }
    
    pub fn federation_error(server: impl Into<String>, message: impl Into<String>) -> Self {
        Self::FederationError { 
            server: server.into(), 
            message: message.into() 
        }
    }
    
    pub fn config_error(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ConfigurationError { 
            field: field.into(), 
            message: message.into() 
        }
    }
    
    pub fn database_error(operation: impl Into<String>, message: impl Into<String>) -> Self {
        Self::DatabaseError { 
            operation: operation.into(), 
            message: message.into() 
        }
    }
    
    pub fn network_error(service: impl Into<String>, message: impl Into<String>) -> Self {
        Self::NetworkError { 
            service: service.into(), 
            message: message.into() 
        }
    }
    
    pub fn validation_error(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ValidationError { 
            field: field.into(), 
            message: message.into() 
        }
    }
    
    pub fn rate_limit_exceeded(resource: impl Into<String>, seconds: u64) -> Self {
        Self::RateLimitExceeded { 
            resource: resource.into(), 
            seconds 
        }
    }
}

/// Conversion from Continuwuity's Error to our EnhancedError
impl From<crate::Error> for EnhancedError {
    fn from(error: crate::Error) -> Self {
        match error {
            crate::Error::Database(msg) => {
                Self::database_error("database_operation", msg.to_string())
            },
            crate::Error::Config(field, msg) => {
                Self::config_error(field, msg.to_string())
            },
            crate::Error::Federation(server, _) => {
                Self::federation_error(server.to_string(), "Federation communication failed")
            },
            crate::Error::Io(io_error) => {
                Self::network_error("io_operation", io_error.to_string())
            },
            _ => {
                Self::database_error("unknown_operation", error.message())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_authentication_error() {
        let error = EnhancedError::auth_failed("Invalid token");
        assert_eq!(error.matrix_error_code(), "M_UNAUTHORIZED");
        assert_eq!(error.http_status_code(), 401);
        assert!(error.user_message().contains("check your login credentials"));
        assert_eq!(error.sanitized_message(), "Authentication failed");
    }
    
    #[test]
    fn test_room_not_found_error() {
        let error = EnhancedError::room_not_found("!room123:example.com");
        assert_eq!(error.matrix_error_code(), "M_NOT_FOUND");
        assert_eq!(error.http_status_code(), 404);
        assert!(error.user_message().contains("could not be found"));
        assert!(error.sanitized_message().contains("!room123:example.com"));
    }
    
    #[test]
    fn test_user_not_found_error() {
        let error = EnhancedError::user_not_found("@user:example.com");
        assert_eq!(error.matrix_error_code(), "M_NOT_FOUND");
        assert_eq!(error.http_status_code(), 404);
        assert!(error.user_message().contains("was not found"));
        assert!(error.sanitized_message().contains("@user:example.com"));
    }
    
    #[test]
    fn test_insufficient_permissions_error() {
        let error = EnhancedError::insufficient_permissions("Admin role required");
        assert_eq!(error.matrix_error_code(), "M_FORBIDDEN");
        assert_eq!(error.http_status_code(), 403);
        assert!(error.user_message().contains("don't have permission"));
        assert_eq!(error.sanitized_message(), "Insufficient permissions");
    }
    
    #[test]
    fn test_federation_error() {
        let error = EnhancedError::federation_error("matrix.org", "Connection timeout");
        assert_eq!(error.matrix_error_code(), "M_FEDERATION_ERROR");
        assert_eq!(error.http_status_code(), 502);
        assert!(error.user_message().contains("Unable to communicate"));
        assert!(error.sanitized_message().contains("matrix.org"));
    }
    
    #[test]
    fn test_configuration_error() {
        let error = EnhancedError::config_error("database_url", "Invalid format");
        assert_eq!(error.matrix_error_code(), "M_UNKNOWN");
        assert_eq!(error.http_status_code(), 500);
        assert!(error.user_message().contains("Server configuration error"));
        assert!(error.sanitized_message().contains("database_url"));
    }
    
    #[test]
    fn test_database_error() {
        let error = EnhancedError::database_error("user_lookup", "Connection lost");
        assert_eq!(error.matrix_error_code(), "M_UNKNOWN");
        assert_eq!(error.http_status_code(), 500);
        assert!(error.user_message().contains("Database operation"));
        assert!(error.sanitized_message().contains("user_lookup"));
    }
    
    #[test]
    fn test_network_error() {
        let error = EnhancedError::network_error("federation", "DNS resolution failed");
        assert_eq!(error.matrix_error_code(), "M_UNKNOWN");
        assert_eq!(error.http_status_code(), 503);
        assert!(error.user_message().contains("currently unavailable"));
        assert!(error.sanitized_message().contains("federation"));
    }
    
    #[test]
    fn test_validation_error() {
        let error = EnhancedError::validation_error("room_name", "Too long");
        assert_eq!(error.matrix_error_code(), "M_BAD_JSON");
        assert_eq!(error.http_status_code(), 400);
        assert!(error.user_message().contains("Invalid value"));
        assert!(error.sanitized_message().contains("room_name"));
    }
    
    #[test]
    fn test_rate_limit_error() {
        let error = EnhancedError::rate_limit_exceeded("room_creation", 60);
        assert_eq!(error.matrix_error_code(), "M_LIMIT_EXCEEDED");
        assert_eq!(error.http_status_code(), 429);
        assert!(error.user_message().contains("Too many requests"));
        assert!(error.sanitized_message().contains("room_creation"));
    }
    
    #[test]
    fn test_error_display() {
        let error = EnhancedError::auth_failed("Token expired");
        let display = format!("{}", error);
        assert!(display.contains("Authentication failed"));
        assert!(display.contains("Token expired"));
    }
    
    #[test]
    fn test_error_debug() {
        let error = EnhancedError::room_not_found("!test:example.com");
        let debug = format!("{:?}", error);
        assert!(debug.contains("RoomNotFound"));
        assert!(debug.contains("!test:example.com"));
    }
}
