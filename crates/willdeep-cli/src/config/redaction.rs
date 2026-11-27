//! Shared config display redaction, kept separate as new tool settings are added.

pub(super) fn redact_inline_secrets(value: &mut toml::Value) {
    if let Some(providers) = value
        .get_mut("providers")
        .and_then(toml::Value::as_table_mut)
    {
        for (_, provider) in providers.iter_mut() {
            redact_field(provider, "api_key");
        }
    }
    if let Some(channels) = value
        .get_mut("notifications")
        .and_then(toml::Value::as_table_mut)
        .and_then(|notifications| notifications.get_mut("channels"))
        .and_then(toml::Value::as_array_mut)
    {
        redact_channels(channels);
    }
    // MCP 段通常已经迁出主配置。迁不走的段仍留在 config.toml 里，
    // `config show` 按原文打印，明文 token 要先换掉。
    if let Some(servers) = value
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
    {
        for (_, server) in servers.iter_mut() {
            redact_field(server, "token");
        }
    }
}

fn redact_channels(channels: &mut Vec<toml::Value>) {
    for channel in channels {
        redact_field(channel, "secret");
        let Some(table) = channel.as_table_mut() else {
            continue;
        };
        if let Some(smtp) = table.get_mut("smtp") {
            redact_field(smtp, "password");
        }
        if let Some(resend) = table.get_mut("resend") {
            redact_field(resend, "api_key");
        }
    }
}

/// Inline secrets are redacted; environment variable references remain visible.
fn redact_field(value: &mut toml::Value, field: &str) {
    if let Some(table) = value.as_table_mut()
        && table.contains_key(field)
    {
        table.insert(
            field.to_owned(),
            toml::Value::String("[REDACTED]".to_owned()),
        );
    }
}
