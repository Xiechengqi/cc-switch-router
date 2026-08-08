use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Gone(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    UnprocessableEntity(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error("{0}")]
    ServiceUnavailable(String),
    #[error("{message}")]
    RateLimited {
        message: String,
        retry_after_secs: u64,
    },
    #[error("{message}")]
    Coded {
        status: StatusCode,
        code: &'static str,
        message: String,
        details: Value,
    },
    #[error("{0}")]
    Internal(String),
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
    #[serde(rename = "errorId", skip_serializing_if = "Option::is_none")]
    error_id: Option<String>,
}

impl AppError {
    pub fn coded_forbidden(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self::Coded {
            status: StatusCode::FORBIDDEN,
            code,
            message: message.into(),
            details,
        }
    }

    pub fn coded_conflict(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self::Coded {
            status: StatusCode::CONFLICT,
            code,
            message: message.into(),
            details,
        }
    }

    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Coded { code, .. } => Some(*code),
            _ => None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let database_unavailable = match &self {
            AppError::ServiceUnavailable(message) | AppError::Internal(message) => {
                message.contains("database unavailable:")
            }
            _ => false,
        };
        let status = match &self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Gone(_) => StatusCode::GONE,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::UnprocessableEntity(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            AppError::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            AppError::Coded { status, .. } => *status,
            AppError::Internal(_) if database_unavailable => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let retry_after_secs = match &self {
            AppError::RateLimited {
                retry_after_secs, ..
            } => Some((*retry_after_secs).max(1)),
            _ => None,
        };
        let server_error = status.is_server_error();
        let error_id = server_error.then(|| uuid::Uuid::new_v4().to_string());
        if let Some(error_id) = error_id.as_deref() {
            tracing::error!(
                error_id,
                status = status.as_u16(),
                error = %self,
                "request failed with server error"
            );
        }
        let (code, details) = match &self {
            _ if server_error => (database_unavailable.then_some("DATABASE_UNAVAILABLE"), None),
            AppError::Coded { code, details, .. } => (Some(*code), Some(details.clone())),
            _ => (None, None),
        };
        let body = ErrorBody {
            message: if database_unavailable {
                "database unavailable".into()
            } else if status == StatusCode::SERVICE_UNAVAILABLE {
                "service unavailable".into()
            } else if server_error {
                "internal error".into()
            } else {
                self.to_string()
            },
            code,
            details,
            error_id,
        };
        if let Some(retry_after_secs) = retry_after_secs {
            return (
                status,
                [(header::RETRY_AFTER, retry_after_secs.to_string())],
                Json(body),
            )
                .into_response();
        }
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_response_includes_retry_after() {
        let response = AppError::RateLimited {
            message: "registration rate limit exceeded".into(),
            retry_after_secs: 42,
        }
        .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "42");
    }

    #[tokio::test]
    async fn coded_error_response_includes_machine_readable_fields() {
        let response = AppError::coded_forbidden(
            "MARKET_ACCESS_REQUIRED",
            "seller approval is required",
            serde_json::json!({ "productKind": "share", "pricingKind": "paid" }),
        )
        .into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read coded error response");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("decode coded error response");
        assert_eq!(payload["message"], "seller approval is required");
        assert_eq!(payload["code"], "MARKET_ACCESS_REQUIRED");
        assert_eq!(payload["details"]["productKind"], "share");
        assert_eq!(payload["details"]["pricingKind"], "paid");
    }

    #[tokio::test]
    async fn wrapped_database_unavailable_error_returns_503() {
        let response = AppError::Internal(
            "write share failed: database unavailable: connection refused".into(),
        )
        .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read database error response");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("decode database error response");
        assert_eq!(payload["message"], "database unavailable");
        assert_eq!(payload["code"], "DATABASE_UNAVAILABLE");
        assert!(
            payload["errorId"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
    }

    #[tokio::test]
    async fn internal_error_is_not_exposed() {
        let response =
            AppError::Internal("query users failed: secret table name".into()).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read internal error response");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("decode internal error response");
        assert_eq!(payload["message"], "internal error");
        assert!(payload["code"].is_null());
        assert!(payload["details"].is_null());
        assert!(
            payload["errorId"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(!String::from_utf8_lossy(&body).contains("secret table name"));
    }
}
