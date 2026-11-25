// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Cloud model servers: hosted APIs that answer the Anthropic Messages API,
//! as opposed to hardware you run. A server is cloud when its URL is on a
//! known cloud host or when you added it with the Cloud service switch on.
//! The switch is kept on the server's record and the host check runs fresh
//! each time, so a server added before either existed turns cloud with no
//! migration. A cloud server's agents get every tool and the model's own
//! output limit.

use maxhq_protocol::local_models::{ModelServer, ModelServerKind};

/// The hosts known to serve the Anthropic Messages API as a hosted service,
/// each matched with its subdomains. Each is the domain of a base URL its
/// owner's docs give for Claude Code:
///
/// - `openrouter.ai`: OpenRouter, `https://openrouter.ai/api`.
/// - `z.ai`: Z.ai, `https://api.z.ai/api/anthropic`.
/// - `bigmodel.cn`: Zhipu, `https://open.bigmodel.cn/api/anthropic`.
/// - `deepseek.com`: DeepSeek, `https://api.deepseek.com/anthropic`.
/// - `moonshot.ai` and `moonshot.cn`: Moonshot's Kimi,
///   `https://api.moonshot.ai/anthropic` and `https://api.moonshot.cn/anthropic`.
/// - `minimax.io` and `minimax.cn`: MiniMax, `https://api.minimax.io/anthropic`
///   and `https://api.minimax.cn/anthropic`.
const KNOWN_HOSTS: &[&str] = &[
    "openrouter.ai",
    "z.ai",
    "bigmodel.cn",
    "deepseek.com",
    "moonshot.ai",
    "moonshot.cn",
    "minimax.io",
    "minimax.cn",
];

/// Whether a base URL is a known cloud host: https, on a host from
/// [`KNOWN_HOSTS`] or a subdomain of one. Plain http never is, since no
/// hosted service answers there.
pub(crate) fn is_known_cloud(base_url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(base_url.trim()) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(url::Host::Domain(host)) = parsed.host() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    KNOWN_HOSTS.iter().any(|known| {
        host == *known
            || host
                .strip_suffix(known)
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

/// Whether a server is cloud: a URL you marked a cloud service, or one on a
/// known cloud host. A managed server, which `maxd` runs, never is.
pub(crate) fn is_cloud(server: &ModelServer) -> bool {
    matches!(server.kind, ModelServerKind::External { .. })
        && (server.cloud || server.base_url.as_deref().is_some_and(is_known_cloud))
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::{DeviceId, ModelServerId};
    use maxhq_protocol::local_models::ServerFlavor;

    use super::*;

    #[test]
    fn known_cloud_hosts_and_their_subdomains_are_cloud() {
        for url in [
            "https://openrouter.ai/api",
            "https://OpenRouter.ai/api/",
            "https://api.z.ai/api/anthropic",
            "https://open.bigmodel.cn/api/anthropic",
            "https://api.deepseek.com/anthropic",
            "https://api.moonshot.ai/anthropic",
            "https://api.moonshot.cn/anthropic",
            "https://api.minimax.io/anthropic",
            "https://api.minimax.cn/anthropic",
            "https://openrouter.ai.:443/api",
        ] {
            assert!(is_known_cloud(url), "{url}");
        }
    }

    #[test]
    fn other_hosts_plain_http_and_lookalikes_are_not() {
        for url in [
            "http://openrouter.ai/api",
            "https://devbox:11434",
            "http://127.0.0.1:8080",
            "https://notopenrouter.ai/api",
            "https://openrouter.ai.example.com/api",
            "https://example.com/openrouter.ai",
            "https://api.anthropic.com",
            "https://[::1]:8080",
            "not a url",
            "",
        ] {
            assert!(!is_known_cloud(url), "{url}");
        }
    }

    fn server(kind: ModelServerKind, url: Option<&str>, cloud: bool) -> ModelServer {
        ModelServer {
            id: ModelServerId::derive("server:example"),
            device: DeviceId::derive("device:example"),
            kind,
            label: "Example".into(),
            base_url: url.map(str::to_owned),
            token: false,
            cloud,
            probe: None,
            models: vec![],
        }
    }

    #[test]
    fn a_server_is_cloud_by_its_host_or_its_switch_and_never_when_managed() {
        let external = ModelServerKind::External {
            flavor: ServerFlavor::Other,
        };
        // A record saved before cloud servers reads its switch as off and
        // still turns cloud on a known host.
        assert!(is_cloud(&server(
            external.clone(),
            Some("https://openrouter.ai/api"),
            false
        )));
        assert!(is_cloud(&server(
            external.clone(),
            Some("https://llm.example.com"),
            true
        )));
        assert!(!is_cloud(&server(
            external,
            Some("http://devbox:11434"),
            false
        )));
        assert!(!is_cloud(&server(ModelServerKind::Managed, None, true)));
    }
}
