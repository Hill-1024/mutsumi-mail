use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub folders: bool,
    pub labels: bool,
    pub idle_push: bool,
    pub server_search: bool,
    pub r#move: bool,
    pub copy: bool,
    pub append: bool,
    pub append_sent: bool,
    pub drafts: bool,
    pub trash: bool,
    pub archive: bool,
    pub flags: bool,
    pub keywords: bool,
    pub threading: bool,
    pub partial_fetch: bool,
    pub smtp_utf8: bool,
    pub oauth2: bool,
    pub multiple_identities: bool,
}
