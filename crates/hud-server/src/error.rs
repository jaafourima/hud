use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use hud_sync::ErrorBody;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized,
    Forbidden(String),
    Conflict(String),
    Internal(String),
}

pub type Result<T> = std::result::Result<T, AppError>;

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "invalid credentials or token".to_owned(),
            ),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m),
            AppError::Internal(m) => {
                // Details stay in the log; clients get nothing they could probe with.
                tracing::error!("internal error: {m}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_owned(),
                )
            }
        };
        (status, Json(ErrorBody { error: msg })).into_response()
    }
}

impl From<tokio_postgres::Error> for AppError {
    fn from(e: tokio_postgres::Error) -> Self {
        use tokio_postgres::error::SqlState;
        // Class 22 (data exception) and 23502 (not null) mean the client sent a bad value.
        match e.code() {
            Some(c) if c.code().starts_with("22") || *c == SqlState::NOT_NULL_VIOLATION => {
                AppError::BadRequest(format!("invalid field value ({})", c.code()))
            }
            _ => AppError::Internal(e.to_string()),
        }
    }
}

impl From<deadpool_postgres::PoolError> for AppError {
    fn from(e: deadpool_postgres::PoolError) -> Self {
        AppError::Internal(e.to_string())
    }
}
