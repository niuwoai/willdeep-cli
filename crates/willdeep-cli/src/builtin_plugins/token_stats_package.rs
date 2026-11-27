use std::path::Path;

use anyhow::{Context, Result};
use rust_embed::RustEmbed;
use serde_json::{Value, json};
use willdeep_core::plugin::{PluginHost, installed_versions};

use super::BuiltinPackage;

pub(super) const PACKAGE: BuiltinPackage = BuiltinPackage {
    id: "willdeep-token-stats",
    short: "token-stats",
    server: "token_stats",
    version: "0.1.0-rc2",
    display_name: "用量统计",
    description: "查看本机 Token 活动、子代理用量、费用日历和单日明细。",
    permissions: &["process.execute"],
    timeout_seconds: 30,
};

#[derive(RustEmbed)]
#[folder = "assets/willdeep-token-stats/ui"]
struct StatsAssets;

pub(super) fn contributions() -> Value {
    json!({
        "destinations": [{
            "id": "token-stats", "titleKey": "destination.stats", "icon": "sf:chart.bar.xaxis",
            "mainPage": "token-stats.main"
        }],
        "pages": [{"id": "token-stats.main", "runtime": "localWeb", "entryPath": "ui/index.html"}],
        "commands": [
            {"id": "token-stats.open", "titleKey": "command.open", "handler": {
                "type": "navigate", "destination": "token-stats"
            }},
            {"id": "token-stats.query", "titleKey": "command.query", "handler": {
                "type": "mcpTool", "server": PACKAGE.server, "tool": "query"
            }}
        ],
        "menus": {"commandPalette": ["token-stats.open"]}
    })
}

pub(super) fn locales() -> Value {
    json!({
        "destination.stats": "用量统计",
        "command.open": "打开用量统计",
        "command.query": "查询用量统计"
    })
}

pub(super) fn write_assets(directory: &Path) -> Result<()> {
    let _entry = StatsAssets::get("index.html").context("missing bundled token stats page")?;
    for name in StatsAssets::iter() {
        let path = directory.join("ui").join(name.as_ref());
        std::fs::create_dir_all(path.parent().context("token stats asset has no parent")?)?;
        let asset = StatsAssets::get(&name).context("missing bundled token stats asset")?;
        std::fs::write(&path, asset.data)
            .with_context(|| format!("write token stats asset {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn ensure_installed(home: &Path) -> Result<()> {
    if installed_versions(&PluginHost::shared_root(home), PACKAGE.id)
        .iter()
        .any(|version| version == PACKAGE.version)
    {
        return Ok(());
    }
    let staging =
        std::env::temp_dir().join(format!("willdeep-token-stats-{}", uuid::Uuid::new_v4()));
    let result = super::write_package(&PACKAGE, &staging)
        .and_then(|()| crate::plugin_cmd::install_one(home, &staging).map(|_| ()));
    if let Err(error) = std::fs::remove_dir_all(&staging)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("warning: token_stats_staging_cleanup_failed error={error}");
    }
    result
}
