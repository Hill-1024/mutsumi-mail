use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccountInput {
    pub email: String,
    pub display_name: String,
    pub provider_id: String,
    pub secret: String,
    #[serde(default)]
    pub incoming_secret: Option<String>,
    #[serde(default)]
    pub outgoing_secret: Option<String>,
    #[serde(default)]
    pub incoming: Option<EndpointConfig>,
    #[serde(default)]
    pub outgoing: Option<EndpointConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointConfig {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub tls_mode: String,
    pub auth_method: String,
    pub username: String,
}
