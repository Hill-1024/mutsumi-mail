use serde::{Deserialize, Serialize};

use crate::domain::capabilities::ProviderCapabilities;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointPreset {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub tls_mode: String,
    pub auth_methods: Vec<String>,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreset {
    pub id: String,
    pub display_name: String,
    pub email_domain_patterns: Vec<String>,
    pub incoming: Option<EndpointPreset>,
    pub outgoing: Option<EndpointPreset>,
    pub help_text: String,
    pub capabilities: ProviderCapabilities,
    pub quirks: Vec<String>,
}

pub fn provider_presets() -> Vec<ProviderPreset> {
    vec![
        ProviderPreset {
            id: "qq".into(), display_name: "QQ 邮箱".into(), email_domain_patterns: vec!["qq.com".into(), "foxmail.com".into()],
            incoming: Some(imap("imap.qq.com", "implicit")), outgoing: Some(smtp("smtp.qq.com", 465, "implicit", "password")),
            help_text: "使用客户端授权码，不是 QQ 登录密码。请先在邮箱设置中开启 IMAP/SMTP。".into(), capabilities: capabilities(true), quirks: vec!["客户端授权码".into(), "完整邮箱地址作为用户名".into()],
        },
        ProviderPreset {
            id: "netease-163".into(), display_name: "网易 163 邮箱".into(), email_domain_patterns: vec!["163.com".into()],
            incoming: Some(imap("imap.163.com", "implicit")), outgoing: Some(smtp("smtp.163.com", 465, "implicit", "password")),
            help_text: "使用客户端授权码，不是网页登录密码。请先在邮箱设置中开启 IMAP/SMTP。".into(), capabilities: capabilities(true), quirks: vec!["客户端授权码".into(), "可能需要重新生成授权码".into()],
        },
        ProviderPreset {
            id: "generic".into(), display_name: "通用 IMAP + SMTP".into(), email_domain_patterns: vec![],
            incoming: Some(EndpointPreset { protocol: "imap".into(), host: String::new(), port: 993, tls_mode: "implicit".into(), auth_methods: vec!["password".into()], username: None }),
            outgoing: Some(EndpointPreset { protocol: "smtp".into(), host: String::new(), port: 465, tls_mode: "implicit".into(), auth_methods: vec!["password".into()], username: None }),
            help_text: "为标准邮件服务器手动填写 IMAP 与 SMTP 端点。当前使用同一密码或授权码验证收发连接。".into(), capabilities: capabilities(true), quirks: vec![],
        },
        ProviderPreset {
            id: "cloudflare-smtp".into(), display_name: "Cloudflare Email Sending".into(), email_domain_patterns: vec!["cloudflare.email".into()],
            incoming: None, outgoing: Some(EndpointPreset { protocol: "smtp".into(), host: "smtp.mx.cloudflare.net".into(), port: 465, tls_mode: "implicit".into(), auth_methods: vec!["api-token".into()], username: Some("api_token".into()) }),
            help_text: "这是 outbound-only 发件 preset。Cloudflare SMTP 用户名固定为 api_token，密码为具有 Email Sending: Edit 权限的 API Token。".into(), capabilities: ProviderCapabilities { smtp_utf8: true, ..Default::default() }, quirks: vec!["仅发件".into(), "SMTP implicit TLS 465".into()],
        },
    ]
}

fn imap(host: &str, tls_mode: &str) -> EndpointPreset {
    EndpointPreset {
        protocol: "imap".into(),
        host: host.into(),
        port: 993,
        tls_mode: tls_mode.into(),
        auth_methods: vec!["password".into()],
        username: None,
    }
}
fn smtp(host: &str, port: u16, tls_mode: &str, auth_method: &str) -> EndpointPreset {
    EndpointPreset {
        protocol: "smtp".into(),
        host: host.into(),
        port,
        tls_mode: tls_mode.into(),
        auth_methods: vec![auth_method.into()],
        username: None,
    }
}
fn capabilities(_folder_ops: bool) -> ProviderCapabilities {
    ProviderCapabilities {
        folders: true,
        flags: true,
        partial_fetch: true,
        threading: true,
        smtp_utf8: true,
        ..Default::default()
    }
}

pub fn detect_provider(email: &str) -> Option<ProviderPreset> {
    let domain = email.rsplit_once('@')?.1.to_ascii_lowercase();
    provider_presets().into_iter().find(|preset| {
        preset
            .email_domain_patterns
            .iter()
            .any(|pattern| domain == *pattern || domain.ends_with(&format!(".{pattern}")))
    })
}

#[cfg(test)]
mod tests {
    use super::{detect_provider, provider_presets};

    #[test]
    fn recognizes_qq_and_163_domains() {
        assert_eq!(
            detect_provider("user@qq.com")
                .as_ref()
                .map(|provider| provider.id.as_str()),
            Some("qq")
        );
        assert_eq!(
            detect_provider("user@163.com")
                .as_ref()
                .map(|provider| provider.id.as_str()),
            Some("netease-163")
        );
        assert!(detect_provider("user@example.test").is_none());
    }

    #[test]
    fn cloudflare_is_outbound_only() {
        let provider = provider_presets()
            .into_iter()
            .find(|provider| provider.id == "cloudflare-smtp")
            .expect("preset");
        assert!(provider.incoming.is_none());
        assert_eq!(
            provider.outgoing.expect("outgoing").host,
            "smtp.mx.cloudflare.net"
        );
        let outgoing = provider_presets()
            .into_iter()
            .find(|provider| provider.id == "cloudflare-smtp")
            .and_then(|provider| provider.outgoing)
            .expect("outgoing preset");
        assert_eq!(outgoing.username.as_deref(), Some("api_token"));
    }
}
