use crate::i18n::Language;
use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::path::Path;
use willdeep_runtime_protocol::web_app::{WebAppStartParams, WebAppStatus};

const DEFAULT_WEBAPP_LISTEN: &str = "127.0.0.1:9847";

pub(super) async fn handle_webapp_command(
    prompt: &str,
    home: &Path,
    workspace: &Path,
    config: Option<&Path>,
    profile: Option<&str>,
    language: Language,
) -> Result<Option<String>> {
    let value = prompt.trim();
    if value != "/webapp" && !value.starts_with("/webapp ") {
        return Ok(None);
    }
    let argument = value.strip_prefix("/webapp").unwrap_or_default().trim();
    let status = if argument.eq_ignore_ascii_case("status") {
        crate::daemon::web_app::remote_status(home).await?
    } else if argument.eq_ignore_ascii_case("stop") {
        crate::daemon::web_app::remote_stop(home).await?
    } else {
        let listen = parse_listen(argument)?;
        let existing = crate::daemon::web_app::remote_status(home).await?;
        if existing.running {
            if !argument.is_empty()
                && !argument.eq_ignore_ascii_case("start")
                && existing.listen != Some(listen)
            {
                bail!(
                    "{}",
                    language.text(
                        "Web 已运行，请先 /webapp stop 再修改监听地址",
                        "Web is running; /webapp stop before changing its address",
                        "Web は実行中です。アドレスを変更する前に /webapp stop を実行してください"
                    )
                );
            }
            existing
        } else {
            crate::daemon::web_app::remote_start(
                home,
                &WebAppStartParams {
                    listen,
                    workspace: workspace.into(),
                    config: Some(std::path::absolute(
                        config
                            .map(|path| path.to_path_buf())
                            .unwrap_or(crate::config::default_config_path()?),
                    )?),
                    profile: profile.map(str::to_owned),
                    language: language.text("zh-CN", "en", "ja").into(),
                },
            )
            .await?
        }
    };
    Ok(Some(status_message(&status, language)))
}

fn parse_listen(argument: &str) -> Result<SocketAddr> {
    let listen: SocketAddr = if argument.is_empty() || argument.eq_ignore_ascii_case("start") {
        DEFAULT_WEBAPP_LISTEN
            .parse()
            .expect("valid default address")
    } else {
        argument
            .parse()
            .context("usage: /webapp [status|start|stop|127.0.0.1:PORT]")?
    };
    if !listen.ip().is_loopback() {
        bail!("/webapp only accepts a loopback address");
    }
    Ok(listen)
}

fn status_message(status: &WebAppStatus, language: Language) -> String {
    let label = if status.running {
        language.text(
            "System: Web App 由 Runtime 托管，退出 CLI 后仍可使用；/webapp stop 才会关闭",
            "System: Web App is hosted by Runtime and survives CLI exit; /webapp stop disables it",
            "System: Web App は Runtime が管理し、CLI 終了後も動作します。/webapp stop で停止します",
        )
    } else if status.enabled {
        language.text(
            "System: Web App 已启用但未运行，请检查 Runtime 状态",
            "System: Web App is enabled but not running; check Runtime status",
            "System: Web App は有効ですが実行されていません。Runtime の状態を確認してください",
        )
    } else {
        language.text(
            "System: Web App 未启动；输入 /webapp 启动",
            "System: Web App is stopped; enter /webapp to start it",
            "System: Web App は停止中です。/webapp で起動できます",
        )
    };
    let address = status
        .listen
        .map(|listen| format!(" · http://{listen}"))
        .unwrap_or_default();
    let error = status
        .error
        .as_deref()
        .map(|error| format!(" · {error}"))
        .unwrap_or_default();
    format!("{label}{address}{error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_loopback_and_uses_runtime_owned_status() {
        assert_eq!(parse_listen("").unwrap().to_string(), DEFAULT_WEBAPP_LISTEN);
        assert!(parse_listen("0.0.0.0:9847").is_err());
        assert!(parse_listen("127.0.0.1:bad").is_err());
        let message = status_message(
            &WebAppStatus {
                enabled: true,
                running: true,
                listen: Some(parse_listen("").unwrap()),
                error: None,
            },
            Language::ZhCn,
        );
        assert!(message.contains("退出 CLI 后仍可使用"));
    }

    #[tokio::test]
    async fn stopped_status_does_not_start_a_runtime() {
        let root = std::env::temp_dir().join(format!("willdeep-webapp-{}", uuid::Uuid::new_v4()));
        let message =
            handle_webapp_command("/webapp status", &root, &root, None, None, Language::ZhCn)
                .await
                .unwrap()
                .unwrap();
        assert!(message.contains("未启动"));
        assert!(!root.exists());
        assert!(
            handle_webapp_command("normal prompt", &root, &root, None, None, Language::En)
                .await
                .unwrap()
                .is_none()
        );
    }
}
