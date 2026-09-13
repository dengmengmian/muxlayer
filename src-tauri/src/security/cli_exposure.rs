//! headless `agentgate-serve` 的暴露面检查(纯函数,供 cli/serve.rs 调用并在 lib 测试覆盖)。
//!
//! - 绑定非回环地址又没开 TLS:本地 token 和全部 prompt 以明文走网络,启动时必须醒目告警。
//! - `provider-add --api-key sk-…`:key 会进 shell history 和 `ps` 输出。支持从环境变量
//!   或 stdin(`--api-key -`)读取,命令行明文传入时告警。

use std::net::IpAddr;

/// 读取 provider API key 的环境变量(优先)与旧名。
pub const PROVIDER_API_KEY_ENV: &str = "MUXLAYER_PROVIDER_API_KEY";
pub const LEGACY_PROVIDER_API_KEY_ENV: &str = "AGENTGATE_API_KEY";

/// host 是否只在本机可达(回环地址 / localhost)。
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// 非回环地址 + 未启用 TLS 时返回告警文本;否则 `None`。
pub fn cleartext_bind_warning(host: &str, tls_enabled: bool) -> Option<String> {
    if tls_enabled || is_loopback_host(host) {
        return None;
    }
    Some(format!(
        "Gateway is listening on non-loopback address {host} WITHOUT TLS: the access token and \
         all prompts/responses travel in cleartext. Bind to 127.0.0.1, or pass --tls-cert and \
         --tls-key, or put it behind a TLS-terminating reverse proxy."
    ))
}

/// API key 的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeySource {
    /// `--api-key <明文>`:会留在 shell history / 进程列表,调用方需告警。
    CommandLine,
    Stdin,
    Env,
}

/// 解析 provider API key:`--api-key -` 读 stdin;`--api-key <值>` 用命令行值;
/// 未传时读 `MUXLAYER_PROVIDER_API_KEY`(旧名 `AGENTGATE_API_KEY`)。
/// `read_stdin` 仅在 `-` 时调用,便于测试注入。
pub fn resolve_api_key(
    cli_value: Option<&str>,
    env_value: Option<String>,
    read_stdin: impl FnOnce() -> std::io::Result<String>,
) -> Result<(String, ApiKeySource), String> {
    let (key, source) = match cli_value {
        Some("-") => (
            read_stdin().map_err(|e| format!("cannot read API key from stdin: {e}"))?,
            ApiKeySource::Stdin,
        ),
        Some(v) => (v.to_string(), ApiKeySource::CommandLine),
        None => match env_value {
            Some(v) => (v, ApiKeySource::Env),
            None => {
                return Err(format!(
                    "API key required: pipe it with `--api-key -`, or set {PROVIDER_API_KEY_ENV}"
                ))
            }
        },
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("API key is empty".to_string());
    }
    Ok((key, source))
}

/// 命令行明文传 key 时的告警文本。
pub fn command_line_key_warning() -> String {
    format!(
        "Warning: passing --api-key on the command line leaves the key in shell history and \
         process listings. Prefer `--api-key -` (read from stdin) or {PROVIDER_API_KEY_ENV}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts_are_not_warned() {
        for host in [
            "127.0.0.1",
            "127.8.9.10",
            "localhost",
            "LOCALHOST",
            "::1",
            "[::1]",
        ] {
            assert!(cleartext_bind_warning(host, false).is_none(), "{host}");
        }
    }

    #[test]
    fn non_loopback_without_tls_is_warned_and_tls_silences_it() {
        for host in ["0.0.0.0", "::", "192.168.1.5", "gateway.example.com"] {
            let w = cleartext_bind_warning(host, false).expect(host);
            assert!(w.contains("WITHOUT TLS"), "{w}");
            assert!(cleartext_bind_warning(host, true).is_none(), "{host}");
        }
    }

    #[test]
    fn api_key_from_stdin_env_or_command_line() {
        let never = || -> std::io::Result<String> { panic!("stdin must not be read") };
        assert_eq!(
            resolve_api_key(Some("sk-cli"), Some("sk-env".into()), never).unwrap(),
            ("sk-cli".to_string(), ApiKeySource::CommandLine)
        );
        assert_eq!(
            resolve_api_key(Some("-"), Some("sk-env".into()), || Ok("sk-stdin\n".into())).unwrap(),
            ("sk-stdin".to_string(), ApiKeySource::Stdin)
        );
        let never = || -> std::io::Result<String> { panic!("stdin must not be read") };
        assert_eq!(
            resolve_api_key(None, Some("sk-env".into()), never).unwrap(),
            ("sk-env".to_string(), ApiKeySource::Env)
        );
        let never = || -> std::io::Result<String> { panic!("stdin must not be read") };
        assert!(resolve_api_key(None, None, never).is_err());
        assert!(resolve_api_key(Some("-"), None, || Ok("  \n".into())).is_err());
    }
}
