//! First-directory approval onboarding; all permissions remain existing modes.
use std::path::Path;

use anyhow::{Context, Result, bail};
use willdeep_core::{ApprovalMode, Session};

use crate::daemon::{RuntimeWorkspace, WorkspaceAccess};
use crate::i18n::Language;

pub(crate) const GUIDE_MODES: [ApprovalMode; 3] = [
    ApprovalMode::Strict,
    ApprovalMode::Smart,
    ApprovalMode::FullAccess,
];

pub(crate) fn needs_guide(
    workspace: &RuntimeWorkspace,
    interactive: bool,
    explicit_mode: Option<ApprovalMode>,
) -> bool {
    interactive
        && !workspace.approval_mode_configured
        && workspace.access != WorkspaceAccess::ReadOnly
        && explicit_mode.is_none()
}

pub(crate) async fn session_override(
    home: &Path,
    session: Option<&Session>,
) -> Result<Option<WorkspaceAccess>> {
    let Some(session) = session else {
        return Ok(None);
    };
    if session.delegation_read_only {
        return Ok(Some(WorkspaceAccess::ReadOnly));
    }
    if !session.runtime_managed {
        return Ok(None);
    }
    Ok(crate::daemon::remote_session_states(home)
        .await?
        .into_iter()
        .find(|state| state.id == session.id)
        .and_then(|state| state.approval_mode))
}

pub(crate) async fn prepare(
    cli: &crate::Cli,
    loaded: &crate::config::LoadedConfig,
    home: &Path,
    resumed: Option<&Session>,
    interactive: bool,
    language: Language,
) -> Result<Option<ApprovalMode>> {
    // Machine/pipe entry points never open a screen or create a new question.
    if !interactive || cli.web_input_json || cli.json || cli.web {
        return Ok(None);
    }
    let root = crate::harness::resolve_workspace(cli, resumed)?;
    let mut workspace = crate::daemon::ensure_remote_workspace(home, &root).await?;
    let session_mode = session_override(home, resumed).await?;
    let configured = if cli.full_auto {
        Some(ApprovalMode::Smart)
    } else {
        loaded
            .file
            .agent
            .approval
            .as_ref()
            .map(|_| crate::harness::configured_approval_mode(&loaded.file.agent))
            .transpose()?
    };
    let explicit = session_mode
        .map(WorkspaceAccess::approval_mode)
        .or(configured);
    if needs_guide(&workspace, interactive, explicit) {
        let Some(mode) = crate::tui::select_workspace_approval(&workspace, language)? else {
            bail!("{}", language.workspace_approval_cancelled());
        };
        workspace = crate::daemon::set_remote_workspace_approval_mode(
            home,
            &root,
            mode,
            mode == ApprovalMode::FullAccess,
        )
        .await
        .context("save directory approval mode")?;
    }
    Ok(Some(
        workspace
            .access
            .with_session_override(explicit.map(WorkspaceAccess::from_approval_mode))
            .approval_mode(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> RuntimeWorkspace {
        RuntimeWorkspace {
            schema: 1,
            id: uuid::Uuid::new_v4(),
            name: "fixture".into(),
            root: "/fixture".into(),
            access: WorkspaceAccess::Smart,
            approval_mode_configured: false,
            provider_profile: None,
            skills: vec![],
            mcp_servers: vec![],
            created_at: 0,
            updated_at: 0,
            active: false,
        }
    }

    #[tokio::test]
    async fn non_tty_startup_never_contacts_runtime_or_opens_a_prompt() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from(["willdeep"]).unwrap();
        let loaded = crate::config::LoadedConfig {
            file: crate::config::ConfigFile::default(),
        };
        let home = std::env::temp_dir().join(format!("unused-approval-{}", uuid::Uuid::new_v4()));
        let mode = prepare(&cli, &loaded, &home, None, false, Language::En)
            .await
            .unwrap();
        assert_eq!(mode, None);
        assert!(!home.exists());
    }

    #[test]
    fn guide_only_for_unconfigured_interactive_directories() {
        let mut workspace = workspace();
        assert!(needs_guide(&workspace, true, None));
        assert!(!needs_guide(&workspace, false, None));
        for mode in ApprovalMode::SELECTABLE {
            assert!(!needs_guide(&workspace, true, Some(mode)));
        }
        workspace.approval_mode_configured = true;
        assert!(!needs_guide(&workspace, true, None));
        workspace.approval_mode_configured = false;
        workspace.access = WorkspaceAccess::ReadOnly;
        assert!(!needs_guide(&workspace, true, None));
    }
}
