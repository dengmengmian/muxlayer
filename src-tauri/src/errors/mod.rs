use serde::Serialize;
use specta::Type;

pub mod codes;

#[derive(Debug, Serialize, Clone, Type)]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub suggestion: Option<String>,
    /// 上游 HTTP 失败信息(网关内部 failover / 熔断判断用,不序列化给前端)。
    /// Box 起来:AppError 是全项目的 Err 类型,内联会超过 clippy result_large_err 阈值。
    #[serde(skip)]
    #[specta(skip)]
    pub upstream: Option<Box<UpstreamFailure>>,
}

/// 上游返回的非 2xx 响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamFailure {
    pub status: u16,
    /// 上游原始响应体。设置后,最后一跳失败时原样回给客户端(保持透传语义)。
    pub body: Option<String>,
}

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
            suggestion: None,
            upstream: None,
        }
    }

    pub fn with_upstream_status(mut self, status: u16) -> Self {
        self.upstream = Some(Box::new(UpstreamFailure { status, body: None }));
        self
    }

    /// 带上上游状态码 + 原始 body:客户端最终拿到的是上游原样响应。
    pub fn with_upstream_response(mut self, status: u16, body: impl Into<String>) -> Self {
        self.upstream = Some(Box::new(UpstreamFailure {
            status,
            body: Some(body.into()),
        }));
        self
    }

    /// 上游 HTTP 状态码(若有)。
    pub fn upstream_status(&self) -> Option<u16> {
        self.upstream.as_ref().map(|u| u.status)
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    pub fn not_found(entity: &str, id: &str) -> Self {
        Self::new("NOT_FOUND", format!("{entity} '{id}' not found"))
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new("VALIDATION_ERROR", message)
    }

    pub fn database(err: rusqlite::Error) -> Self {
        Self::new("DATABASE_ERROR", "Database operation failed").with_detail(err.to_string())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("INTERNAL_ERROR", message)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        Self::database(err)
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        Self::new("NETWORK_ERROR", "Network request failed").with_detail(err.to_string())
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_error_new() {
        let err = AppError::new("TEST_CODE", "test message");
        assert_eq!(err.code, "TEST_CODE");
        assert_eq!(err.message, "test message");
        assert!(err.detail.is_none());
        assert!(err.suggestion.is_none());
    }

    #[test]
    fn test_app_error_with_detail() {
        let err = AppError::new("TEST", "msg").with_detail("extra info");
        assert_eq!(err.detail, Some("extra info".to_string()));
    }

    #[test]
    fn test_app_error_with_suggestion() {
        let err = AppError::new("TEST", "msg").with_suggestion("try this");
        assert_eq!(err.suggestion, Some("try this".to_string()));
    }

    #[test]
    fn test_app_error_not_found() {
        let err = AppError::not_found("Provider", "123");
        assert_eq!(err.code, "NOT_FOUND");
        assert_eq!(err.message, "Provider '123' not found");
    }

    #[test]
    fn test_app_error_validation() {
        let err = AppError::validation("invalid input");
        assert_eq!(err.code, "VALIDATION_ERROR");
        assert_eq!(err.message, "invalid input");
    }

    #[test]
    fn test_app_error_internal() {
        let err = AppError::internal("something broke");
        assert_eq!(err.code, "INTERNAL_ERROR");
        assert_eq!(err.message, "something broke");
    }

    #[test]
    fn upstream_fields_are_not_serialized() {
        let err = AppError::new("UPSTREAM_NON_STREAM_ERROR", "HTTP 429")
            .with_upstream_response(429, "{\"error\":1}");
        assert_eq!(err.upstream_status(), Some(429));
        let v = serde_json::to_value(&err).unwrap();
        assert!(v.get("upstream").is_none());
        // AppError 是全项目 Err 类型,必须保持在 clippy result_large_err 阈值以下。
        assert!(std::mem::size_of::<AppError>() < 128);
    }

    #[test]
    fn test_display_format() {
        let err = AppError::new("CODE", "message");
        assert_eq!(format!("{}", err), "[CODE] message");
    }

    #[test]
    fn test_from_rusqlite_error() {
        let sqlite_err = rusqlite::Error::InvalidQuery;
        let err: AppError = sqlite_err.into();
        assert_eq!(err.code, "DATABASE_ERROR");
    }
}
