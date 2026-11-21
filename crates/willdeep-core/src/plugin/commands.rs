use serde::Serialize;
use serde_json::Value;

use super::manifest::is_valid_id;
use super::{CommandHandler, CommandOutcome, HostError, PluginHost};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RegisteredCommand {
    pub plugin_id: String,
    pub command_id: String,
    pub slash_command: String,
    pub description: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("expected /plugin:<plugin-id>:<command-id>")]
    InvalidSlashCommand,
    #[error("command arguments must be a JSON object")]
    InvalidArguments,
    #[error("plugin `{0}` requires approval again before commands can run")]
    ApprovalRequired(String),
    #[error("this command requires a graphical plugin host")]
    RequiresUi,
    #[error(transparent)]
    Host(#[from] HostError),
}

pub fn parse_slash_command(value: &str) -> Result<(String, String), CommandError> {
    let (plugin, command) = value
        .strip_prefix("/plugin:")
        .and_then(|value| value.split_once(':'))
        .ok_or(CommandError::InvalidSlashCommand)?;
    if !is_valid_id(plugin) || !is_valid_id(command) {
        return Err(CommandError::InvalidSlashCommand);
    }
    Ok((plugin.to_owned(), command.to_owned()))
}

pub fn parse_arguments(value: &str) -> Result<Value, CommandError> {
    let value = if value.trim().is_empty() { "{}" } else { value };
    let arguments: Value =
        serde_json::from_str(value).map_err(|_| CommandError::InvalidArguments)?;
    if !arguments.is_object() {
        return Err(CommandError::InvalidArguments);
    }
    Ok(arguments)
}

impl PluginHost {
    pub async fn registered_commands(
        &self,
        locale: &str,
    ) -> Result<Vec<RegisteredCommand>, CommandError> {
        let mut commands = Vec::new();
        for package in self.packages() {
            if !self.is_enabled(&package.id).await
                || self.approval_gap(&package.id).await?.is_some()
            {
                continue;
            }
            if let Some(manifest) = &package.manifest {
                for command in &manifest.commands {
                    if matches!(command.handler, CommandHandler::McpTool { .. }) {
                        commands.push(RegisteredCommand {
                            plugin_id: package.id.clone(),
                            command_id: command.id.clone(),
                            slash_command: format!("/plugin:{}:{}", package.id, command.id),
                            description: package.localized(&command.title_key, locale),
                        });
                    }
                }
            }
        }
        commands.sort_by(|a, b| a.slash_command.cmp(&b.slash_command));
        Ok(commands)
    }

    pub async fn execute_registered_command(
        &self,
        plugin_id: &str,
        command_id: &str,
        arguments: Value,
    ) -> Result<CommandOutcome, CommandError> {
        if !arguments.is_object() {
            return Err(CommandError::InvalidArguments);
        }
        let package = self.package(plugin_id)?;
        if !self.is_enabled(plugin_id).await {
            return Err(HostError::NotEnabled(plugin_id.to_owned()).into());
        }
        if self.approval_gap(plugin_id).await?.is_some() {
            return Err(CommandError::ApprovalRequired(plugin_id.to_owned()));
        }
        let command = package
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.command(command_id))
            .ok_or_else(|| HostError::UnknownContribution {
                plugin: plugin_id.to_owned(),
                kind: "command",
                id: command_id.to_owned(),
            })?;
        if !matches!(command.handler, CommandHandler::McpTool { .. }) {
            return Err(CommandError::RequiresUi);
        }
        Ok(self
            .execute_command(plugin_id, command_id, arguments)
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::test_support::{
        events, install_fake_plugin, python_available, scratch_home,
    };
    use serde_json::json;
    use std::path::Path;

    fn install(home: &Path, id: &str) {
        install_fake_plugin(home, id, r#"["process.execute"]"#, r#"["srv"]"#);
        let root = home.join("plugins").join(id).join("1.0.0");
        std::fs::write(root.join(".willdeep-plugin/plugin.json"), json!({
            "schemaVersion": 1, "permissions": ["process.execute"],
            "dependencies": {"mcpServers": ["srv"]},
            "contributes": {"destinations": [], "pages": [], "commands": [
                {"id":"echo", "titleKey":"echo", "handler":{"type":"mcpTool","server":"srv","tool":"echo"}},
                {"id":"refresh", "titleKey":"refresh", "handler":{"type":"host","action":"plugin.refresh"}}
            ]}
        }).to_string()).unwrap();
        std::fs::write(
            root.join(".willdeep-plugin/locales/en.json"),
            r#"{"echo":"Echo arguments","refresh":"Refresh"}"#,
        )
        .unwrap();
    }

    #[test]
    fn extension_commands_parse_names_and_object_arguments() {
        assert_eq!(
            parse_slash_command("/plugin:demo:echo").unwrap(),
            ("demo".into(), "echo".into())
        );
        for value in [
            "/help",
            "/plugin::echo",
            "/plugin:demo:echo:extra",
            "/plugin:demo:bad name",
        ] {
            assert!(parse_slash_command(value).is_err());
        }
        assert_eq!(parse_arguments("").unwrap(), json!({}));
        assert_eq!(
            parse_arguments(r#"{"text":"hello 世界"}"#).unwrap()["text"],
            "hello 世界"
        );
        for value in ["null", "[]", "1", "{bad}", "{} {}"] {
            assert!(parse_arguments(value).is_err());
        }
    }

    #[tokio::test]
    async fn extension_commands_discovery_is_namespaced_and_approval_bound() {
        let home = scratch_home("extension-commands-discovery");
        install(&home, "one");
        install(&home, "two");
        let host = PluginHost::discover(&home).unwrap();
        assert!(host.registered_commands("en").await.unwrap().is_empty());
        for id in ["one", "two"] {
            host.approve(id, 1).await.unwrap();
            host.set_enabled(id, true).await.unwrap().unwrap();
        }
        let commands = host.registered_commands("en").await.unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].slash_command, "/plugin:one:echo");
        assert_eq!(commands[1].slash_command, "/plugin:two:echo");
        assert_eq!(commands[0].description, "Echo arguments");
        host.set_enabled("two", false).await.unwrap().unwrap();
        assert_eq!(host.registered_commands("en").await.unwrap().len(), 1);
        assert!(matches!(
            host.execute_registered_command("two", "echo", json!({}))
                .await,
            Err(CommandError::Host(HostError::NotEnabled(_)))
        ));
        std::fs::write(home.join("plugins/one/1.0.0/server.py"), "changed").unwrap();
        assert!(host.registered_commands("en").await.unwrap().is_empty());
        assert!(matches!(
            host.execute_registered_command("one", "echo", json!({}))
                .await,
            Err(CommandError::ApprovalRequired(_))
        ));
        assert!(!home.join("fake-one.jsonl").exists());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn extension_commands_reject_invalid_calls_before_starting_mcp() {
        let home = scratch_home("extension-commands-reject");
        install(&home, "demo");
        let host = PluginHost::discover(&home).unwrap();
        host.approve("demo", 1).await.unwrap();
        host.set_enabled("demo", true).await.unwrap().unwrap();
        assert!(matches!(
            host.execute_registered_command("demo", "echo", Value::Null)
                .await,
            Err(CommandError::InvalidArguments)
        ));
        assert!(matches!(
            host.execute_registered_command("demo", "refresh", json!({}))
                .await,
            Err(CommandError::RequiresUi)
        ));
        assert!(
            host.execute_registered_command("demo", "missing", json!({}))
                .await
                .is_err()
        );
        assert!(!home.join("fake-demo.jsonl").exists());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn extension_commands_execute_real_mcp_with_literal_arguments() {
        if !python_available() {
            return;
        }
        let home = scratch_home("extension-commands-execute");
        install(&home, "demo");
        let host = PluginHost::discover(&home).unwrap();
        host.approve("demo", 1).await.unwrap();
        host.set_enabled("demo", true).await.unwrap().unwrap();
        let arguments = json!({"text":"$(touch should-not-run); 世界", "count":3});
        let CommandOutcome::Tool(result) = host
            .execute_registered_command("demo", "echo", arguments.clone())
            .await
            .unwrap()
        else {
            panic!("expected tool result")
        };
        let echoed: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(echoed, arguments);
        assert_eq!(events(&home.join("fake-demo.jsonl"), "tools_call").len(), 1);
        drop(host);
        std::fs::remove_dir_all(home).unwrap();
    }
}
