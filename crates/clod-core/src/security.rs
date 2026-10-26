//! Pure local-service and child-environment policy.
/// Only exact loopback names/IP literals, optionally followed by a numeric port.
pub fn loopback_host(host: &str) -> bool {
    let (name, port) = if host.starts_with('[') {
        let Some(end) = host.find(']') else {
            return false;
        };
        (&host[..=end], &host[end + 1..])
    } else {
        host.find(':').map_or((host, ""), |at| host.split_at(at))
    };
    (name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1" || name == "[::1]")
        && (port.is_empty()
            || port.strip_prefix(':').is_some_and(|p| {
                !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) && p.parse::<u16>().is_ok()
            }))
}
pub fn upstream_url(url: &str) -> bool {
    url.strip_prefix("http://").is_some_and(|rest| {
        let authority = rest.split('/').next().unwrap_or("");
        loopback_host(authority) && !rest.contains(['?', '#', '@'])
    })
}
pub fn inbound_allowed(origin_present: bool, host: Option<&str>, allow_any_host: bool) -> bool {
    !origin_present && (allow_any_host || host.is_some_and(loopback_host))
}
pub fn scrub_child_env(name: &str) -> bool {
    name.starts_with("CLOD_")
        || name.starts_with("ANTHROPIC_")
        || matches!(name, "CLAUDE_CODE_USE_BEDROCK" | "CLAUDE_CODE_USE_VERTEX")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_service_policy() {
        for host in ["localhost", "LOCALHOST:8787", "127.0.0.1:1", "[::1]:65535"] {
            assert!(loopback_host(host));
        }
        for host in [
            "evil.test",
            "localhost.evil",
            "localhost@evil",
            "127.0.0.2",
            "::1",
            "localhost:",
            "localhost:65536",
        ] {
            assert!(!loopback_host(host));
        }
        assert!(!inbound_allowed(true, Some("localhost"), true));
        assert!(!inbound_allowed(false, None, false));
        assert!(inbound_allowed(false, Some("other"), true));
        assert!(upstream_url("http://[::1]:42"));
        for url in [
            "https://localhost",
            "http://evil",
            "http://localhost@evil",
            "http://localhost?x",
        ] {
            assert!(!upstream_url(url));
        }
    }
    #[test]
    fn environment_policy() {
        for key in [
            "CLOD_API_KEY",
            "CLOD_MEASURE_LOG",
            "ANTHROPIC_MODEL",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
        ] {
            assert!(scrub_child_env(key));
        }
        for key in ["HOME", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY"] {
            assert!(!scrub_child_env(key));
        }
    }
}
