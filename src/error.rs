//! 获取新闻源时的错误类型。

/// 单个新闻源在获取或解析过程中出现的错误。
///
/// `Display` 文本都同时包含源名称与失败原因，便于直接写入标准错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// 网络层错误（连接失败、TLS 握手失败等）。
    #[error("源 {source_name} 请求失败：{reason}")]
    Http {
        /// 源名称。
        source_name: String,
        /// 底层错误描述。
        reason: String,
    },
    /// 返回了非 2xx 状态码。
    #[error("源 {source_name} 返回非 2xx 状态码：{status}")]
    Status {
        /// 源名称。
        source_name: String,
        /// HTTP 状态码。
        status: u16,
    },
    /// 请求超时。
    #[error("源 {source_name} 请求超时（{timeout_secs} 秒）")]
    Timeout {
        /// 源名称。
        source_name: String,
        /// 超时时长，单位秒。
        timeout_secs: u64,
    },
    /// 内容无法解析为 RSS / Atom。
    #[error("源 {source_name} 内容解析失败：{reason}")]
    Parse {
        /// 源名称。
        source_name: String,
        /// 解析失败原因。
        reason: String,
    },
}

impl SourceError {
    /// 返回出错源的名称。
    pub fn source_name(&self) -> &str {
        match self {
            SourceError::Http { source_name, .. }
            | SourceError::Status { source_name, .. }
            | SourceError::Timeout { source_name, .. }
            | SourceError::Parse { source_name, .. } => source_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SourceError;

    #[test]
    fn http_error_shows_source_and_reason() {
        let err = SourceError::Http {
            source_name: "IT之家".to_string(),
            reason: "连接被重置".to_string(),
        };
        let text = err.to_string();
        assert!(text.contains("IT之家"), "缺少源名称：{text}");
        assert!(text.contains("连接被重置"), "缺少原因：{text}");
    }

    #[test]
    fn status_error_shows_source_and_status() {
        let err = SourceError::Status {
            source_name: "Hacker News".to_string(),
            status: 503,
        };
        let text = err.to_string();
        assert!(text.contains("Hacker News"), "缺少源名称：{text}");
        assert!(text.contains("503"), "缺少状态码：{text}");
    }

    #[test]
    fn timeout_error_shows_source_and_reason() {
        let err = SourceError::Timeout {
            source_name: "IT之家".to_string(),
            timeout_secs: 10,
        };
        let text = err.to_string();
        assert!(text.contains("IT之家"), "缺少源名称：{text}");
        assert!(text.contains("超时"), "缺少超时说明：{text}");
        assert!(text.contains("10"), "缺少超时时长：{text}");
    }

    #[test]
    fn parse_error_shows_source_and_reason() {
        let err = SourceError::Parse {
            source_name: "Hacker News".to_string(),
            reason: "不是 RSS 或 Atom".to_string(),
        };
        let text = err.to_string();
        assert!(text.contains("Hacker News"), "缺少源名称：{text}");
        assert!(text.contains("不是 RSS 或 Atom"), "缺少原因：{text}");
    }

    #[test]
    fn error_exposes_source_name() {
        let err = SourceError::Timeout {
            source_name: "IT之家".to_string(),
            timeout_secs: 10,
        };
        assert_eq!(err.source_name(), "IT之家");
    }
}
