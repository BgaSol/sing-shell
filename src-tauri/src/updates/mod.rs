pub mod application;
pub mod network;
pub mod release;
pub mod service;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Component {
    App,
    Engine,
}
impl Component {
    pub fn id(self) -> &'static str {
        match self {
            Self::App => "sing-shell.app",
            Self::Engine => "sing-shell.engine",
        }
    }
    pub fn cache(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Engine => "engine",
        }
    }
}
