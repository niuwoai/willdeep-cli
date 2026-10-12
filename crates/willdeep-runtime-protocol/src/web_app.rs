use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebAppStartParams {
    pub listen: SocketAddr,
    pub workspace: PathBuf,
    pub config: Option<PathBuf>,
    pub profile: Option<String>,
    pub language: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebAppStatus {
    pub enabled: bool,
    pub running: bool,
    pub listen: Option<SocketAddr>,
    pub error: Option<String>,
}
