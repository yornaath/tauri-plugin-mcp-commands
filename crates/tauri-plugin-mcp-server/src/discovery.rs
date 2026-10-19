//! The discovery file, `<app_data_dir>/mcp-server/mcp.json`: how local tools
//! such as the stdio bridge find a running server. Owner-only, written on
//! start and removed on stop.

use std::{io, path::Path};

use serde::{Deserialize, Serialize};

use crate::secure_fs;

pub const DIR_NAME: &str = "mcp-server";
pub const FILE_NAME: &str = "mcp.json";

/// Contents of `mcp.json`. The bridge reads the same format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    /// Format version; bumped on incompatible changes.
    pub version: u32,
    pub url: String,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub app_name: String,
    pub identifier: String,
}

pub(crate) const FORMAT_VERSION: u32 = 1;

pub(crate) fn write(dir: &Path, d: &Discovery) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(d).map_err(io::Error::other)?;
    secure_fs::write_private(&dir.join(FILE_NAME), &json)
}

pub(crate) fn remove(dir: &Path) -> io::Result<()> {
    match std::fs::remove_file(dir.join(FILE_NAME)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
