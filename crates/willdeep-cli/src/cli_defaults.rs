//! Default options for daemon and Web paths that must not parse argv.
use std::path::PathBuf;

use crate::Cli;

impl Cli {
    /// The value clap produces for a bare `willdeep` invocation.
    ///
    /// Building the full command tree needs well over a megabyte of stack in
    /// debug builds, more than a Windows worker thread has left once daemon
    /// request futures are on it. Daemon and Web paths that only need the
    /// default options build them directly instead of parsing argv.
    pub(crate) fn bare() -> Self {
        let variable = |name: &str| std::env::var(name).ok();
        Self {
            command: None,
            prompt: Vec::new(),
            config: std::env::var_os("WILLDEEP_CONFIG").map(PathBuf::from),
            profile: None,
            api_base: variable("WILLDEEP_API_BASE"),
            api_key: variable("WILLDEEP_API_KEY"),
            model: variable("WILLDEEP_MODEL"),
            provider: None,
            api: None,
            workspace: None,
            web_workspaces: Vec::new(),
            full_auto: false,
            max_turns: None,
            max_output_tokens: None,
            json: false,
            resume: None,
            new_session: false,
            session_startup: None,
            list_sessions: false,
            list_projects: false,
            project: None,
            list_approvals: false,
            clear_approvals: false,
            no_tui: false,
            onboarding: false,
            web: false,
            listen: std::net::SocketAddr::from(([127, 0, 0, 1], 9847)),
            language: variable("WILLDEEP_LANGUAGE"),
            web_input_json: false,
        }
    }
}
