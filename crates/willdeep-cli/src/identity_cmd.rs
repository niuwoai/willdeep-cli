//! `willdeep identity` 与 `willdeep fed` 两个子命令。
//!
//! 放在单独模块里有两个原因：`main.rs` 已经贴着 3000 行的上限，而这两个命令
//! 只管身份文件与联邦联系人表，跟 CLI 的启动流程没有关系。

use anyhow::{Result, bail};

use crate::{federation, identity};
use clap::Subcommand;

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum FedAction {
    /// List paired peers.
    List,
    /// Print a one-time invitation link.
    Invite,
    /// Join a peer with its invitation link.
    Join { url: String },
    /// Send a message to a peer.
    Say {
        peer: String,
        #[arg(value_name = "TEXT", num_args = 1.., trailing_var_arg = true)]
        text: Vec<String>,
    },
    /// Forget a peer. Messages already delivered stay in their sessions.
    Remove { peer: String },
}

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum IdentityAction {
    /// Print this instance's id and name.
    Show,
    /// Rename this instance. The id stays the same.
    Rename { name: String },
    /// List the built-in first-run names.
    Names,
}

/// `willdeep fed`：配对、列联系人和发消息。联邦路由挂在公网入口上，所以邀请
/// 链接里的基地址取 `[intake] public_url`。
pub(crate) async fn federation_command(
    action: Option<FedAction>,
    home: &std::path::Path,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    let action = action.unwrap_or(FedAction::List);
    match action {
        FedAction::List => {
            let store = federation::load(home)?;
            if store.contacts.is_empty() {
                println!(
                    "no peers yet; run `willdeep fed invite` on this machine, or `willdeep fed join <url>` with a peer's invitation"
                );
                return Ok(());
            }
            let identity = identity::load(home)?;
            println!("this: {} ({})", identity.name, identity.id);
            for contact in store.contacts {
                println!(
                    "{}  {}  {}  session={}  auto_turns={}",
                    contact.name,
                    identity::short_id(contact.id),
                    contact.url,
                    contact
                        .session_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "none".to_owned()),
                    contact.auto_turns
                );
            }
            Ok(())
        }
        FedAction::Invite => {
            let public_url = read_intake_public_url(config_path)?;
            if public_url.is_empty() {
                bail!("[intake] public_url is empty: peers could not reach this machine");
            }
            let created = federation::create_invite(home)?;
            println!("{}", federation::invite_url(&public_url, &created.token));
            eprintln!(
                "One-time invitation, valid for {} minutes. It carries no secret by itself; a peer that uses it gets its own shared key.",
                (created.expires_at.saturating_sub(federation::now_seconds())) / 60
            );
            Ok(())
        }
        FedAction::Join { url } => {
            let identity = identity::load(home)?;
            let public_url = read_intake_public_url(config_path)?;
            if public_url.is_empty() {
                bail!("[intake] public_url is empty: the peer could not reach this machine back");
            }
            let contact = federation::join(&url, &identity, &public_url).await?;
            let stored = federation::upsert_contact(home, contact)?;
            println!("paired with {} ({})", stored.name, stored.url);
            Ok(())
        }
        FedAction::Say { peer, text } => {
            let text = text.join(" ");
            let store = federation::load(home)?;
            let contact = federation::find(&store, &peer)?.clone();
            let identity = identity::load(home)?;
            federation::send_message_as(identity.id, &contact, &text).await?;
            federation::note_manual_message(home, contact.id)?;
            println!("sent to {}", contact.name);
            Ok(())
        }
        FedAction::Remove { peer } => {
            let contact = federation::remove_contact(home, &peer)?;
            println!("removed {} ({})", contact.name, contact.id);
            Ok(())
        }
    }
}

/// `[intake] public_url`，去掉末尾斜杠。联邦的邀请与加入都以它为基地址——
/// 回环地址拼出来的链接只能自己打开。
pub(crate) fn read_intake_public_url(config_path: Option<&std::path::Path>) -> Result<String> {
    let loaded = crate::config::LoadedConfig::load(config_path)?;
    Ok(loaded
        .file
        .intake
        .public_url
        .trim()
        .trim_end_matches('/')
        .to_owned())
}

/// `willdeep identity`：打印或改名。第一次运行会顺手生成身份——这也是
/// 「首次运行给一个名字」的入口之一（TUI 与 Web 首次打开时同样会生成）。
pub(crate) fn identity_command(
    action: Option<IdentityAction>,
    home: &std::path::Path,
) -> Result<()> {
    match action.unwrap_or(IdentityAction::Show) {
        IdentityAction::Show => {
            let identity = identity::load(home)?;
            // 还没有联系人表，所以不可能撞名；走同一条渲染规则，P1 才需要判重。
            println!("name: {}", identity.display_name(false));
            println!("id: {}", identity.id);
            Ok(())
        }
        IdentityAction::Rename { name } => {
            let identity = identity::rename(home, &name)?;
            println!("name: {}", identity.display_name(false));
            println!("id: {}", identity.id);
            Ok(())
        }
        IdentityAction::Names => {
            println!("{}", identity::NAME_POOL.join(" "));
            Ok(())
        }
    }
}
