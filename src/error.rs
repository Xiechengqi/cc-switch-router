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
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let retry_after_secs = match &self {
            AppError::RateLimited {
                retry_after_secs, ..
            } => Some((*retry_after_secs).max(1)),
            _ => None,
        };
        let (code, details) = match &self {
            AppError::Coded { code, details, .. } => (Some(*code), Some(details.clone())),
            _ => (None, None),
        };
        let body = ErrorBody {
            message: self.to_string(),
            code,
            details,
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
}
