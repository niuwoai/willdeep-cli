use std::path::Path;

use anyhow::{Result, bail};
use willdeep_core::plugin::{CommandOutcome, PluginHost, commands};

pub(super) fn matches(prompt: &str) -> bool {
    let head = prompt.split_whitespace().next().unwrap_or_default();
    head == "/plugin" || head.starts_with("/plugin:")
}

pub(super) async fn execute(home: &Path, prompt: &str, locale: &str) -> Result<String> {
    let value = prompt.trim();
    let (head, arguments) = value.split_once(char::is_whitespace).unwrap_or((value, ""));
    if head == "/plugin" {
        if !arguments.trim().is_empty() {
            bail!("Usage: /plugin, or /plugin:<plugin-id>:<command-id> {{JSON arguments}}");
        }
        let host = PluginHost::discover(home)?;
        let commands = host.registered_commands(locale).await?;
        let mut lines =
            vec!["System: Plugin commands (JSON object arguments; default {})".to_owned()];
        if commands.is_empty() {
            lines.push("No executable plugin commands. Approve and enable a plugin with an MCP command first.".to_owned());
        }
        lines.extend(
            commands
                .into_iter()
                .map(|command| format!("{}  {}", command.slash_command, command.description)),
        );
        return Ok(lines.join("\n"));
    }
    let (plugin, command) = commands::parse_slash_command(head)?;
    let arguments = commands::parse_arguments(arguments)?;
    let host = PluginHost::discover(home)?;
    let outcome = host
        .execute_registered_command(&plugin, &command, arguments)
        .await?;
    match outcome {
        CommandOutcome::Tool(result) => {
            let prefix = if result.get("isError").and_then(serde_json::Value::as_bool) == Some(true)
            {
                "Error"
            } else {
                "System"
            };
            Ok(format!(
                "{prefix}: {head}\n{}",
                serde_json::to_string_pretty(&result)?
            ))
        }
        _ => bail!("This command requires a graphical plugin host"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_commands_route_only_the_plugin_namespace() {
        assert!(matches(" /plugin "));
        assert!(matches("/plugin:demo:echo {\"a\":1}"));
        assert!(!matches("/plugins"));
        assert!(!matches("Explain /plugin:demo:echo"));
        assert_eq!(
            super::super::busy_input("/plugin:demo:echo {}"),
            super::super::BusyInput::Refuse
        );
    }

    #[tokio::test]
    async fn extension_commands_tui_discovery_and_validation() {
        let home = willdeep_core::plugin::test_support::scratch_home("tui-extension-commands");
        let output = execute(&home, "/plugin", "en").await.unwrap();
        assert!(output.contains("No executable plugin commands"));
        assert!(execute(&home, "/plugin unexpected", "en").await.is_err());
        assert!(
            execute(&home, "/plugin:demo:echo []", "en")
                .await
                .unwrap_err()
                .to_string()
                .contains("JSON object")
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
