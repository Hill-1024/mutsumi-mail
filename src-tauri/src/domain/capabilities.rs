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

impl ProviderCapabilities {
    pub fn enabled_count(&self) -> usize {
        [
            self.folders,
            self.labels,
            self.idle_push,
            self.server_search,
            self.r#move,
            self.copy,
            self.append,
            self.append_sent,
            self.drafts,
            self.trash,
            self.archive,
            self.flags,
            self.keywords,
            self.threading,
            self.partial_fetch,
            self.smtp_utf8,
            self.oauth2,
            self.multiple_identities,
        ]
        .into_iter()
        .filter(|enabled| *enabled)
        .count()
    }
}
