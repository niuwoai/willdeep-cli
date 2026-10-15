//! Portable agent definitions. Content identity never grants runtime authority.
pub(crate) mod canonical;
pub mod store;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SCHEMA: &str = "willdeep.agent-definition.v1";
pub const MAX_PACKAGE_BYTES: usize = 256 * 1024;
pub const SIGNING_DOMAIN: &str = "willdeep.agent-package.v1\n";
pub const CAPABILITIES: [&str; 4] = [
    "workspace.read",
    "workspace.edit",
    "process.verify",
    "repository.inspect",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDefinition {
    pub schema: String,
    pub definition_id: String,
    pub version: String,
    pub name: BTreeMap<String, String>,
    pub purpose: String,
    pub base_role: String,
    pub instructions: String,
    pub input_modalities: Vec<String>,
    pub tools: Vec<String>,
    pub skills: Vec<DefinitionSkill>,
    pub forked_from: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionSkill {
    pub id: String,
    pub content: String,
    pub sha256: String,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSignature {
    pub key_id: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPackage {
    pub definition: AgentDefinition,
    pub sha256: String,
    pub signature: Option<PackageSignature>,
}

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && !id.contains("..")
        && (id.as_bytes()[0].is_ascii_lowercase() || id.as_bytes()[0].is_ascii_digit())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._/-".contains(&b))
}

pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl AgentDefinition {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_PACKAGE_BYTES,
            "agent definition is too large"
        );
        let value = canonical::parse_unique(bytes)?;
        ensure!(
            value.get("forked_from").is_some(),
            "forked_from field is required"
        );
        let definition: Self = serde_json::from_value(value)?;
        definition.validate()?;
        Ok(definition)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == SCHEMA, "unsupported agent schema");
        ensure!(valid_id(&self.definition_id), "invalid definition id");
        let version = regex::Regex::new(
            r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc[1-9][0-9]*)?$",
        )?;
        ensure!(version.is_match(&self.version), "invalid agent version");
        ensure!(
            self.name.contains_key("en") || self.name.contains_key("zh-Hans"),
            "agent display name is required"
        );
        ensure!(
            self.name.len() <= 16
                && self.name.iter().all(|(locale, text)| locale.len() <= 32
                    && locale
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && bounded(text, 128)),
            "invalid display names"
        );
        ensure!(
            bounded(&self.purpose, 1024) && bounded(&self.instructions, 16000),
            "invalid agent instructions"
        );
        ensure!(
            crate::subagent::PUBLIC_SUBAGENT_IDS.contains(&self.base_role.as_str()),
            "unsupported base role"
        );
        unique_allowed(&self.input_modalities, &["text", "image"])?;
        ensure!(
            !self.input_modalities.is_empty(),
            "input modalities are required"
        );
        unique_allowed(&self.tools, &CAPABILITIES)?;
        ensure!(self.skills.len() <= 8, "too many skills");
        let mut ids = BTreeSet::new();
        for skill in &self.skills {
            ensure!(
                valid_id(&skill.id) && ids.insert(&skill.id),
                "invalid or duplicate skill id"
            );
            ensure!(
                skill.content.chars().count() <= 24000
                    && (!skill.required || !skill.content.trim().is_empty()),
                "required skill is unavailable"
            );
            ensure!(
                valid_hash(&skill.sha256) && digest(skill.content.as_bytes()) == skill.sha256,
                "skill content hash mismatch"
            );
        }
        ensure!(
            self.forked_from.as_deref().is_none_or(valid_hash),
            "invalid fork revision"
        );
        let json = serde_json::to_string(self)?;
        ensure!(
            json.len() <= MAX_PACKAGE_BYTES,
            "agent definition is too large"
        );
        let credentials =
            regex::Regex::new(r"(?i)(sk-[a-z0-9_-]{20,}|-----BEGIN [A-Z ]*PRIVATE KEY-----)")?;
        ensure!(
            !credentials.is_match(&json),
            "credentials must not be included in agent definitions"
        );
        Ok(())
    }

    pub fn revision(&self) -> Result<String> {
        self.validate()?;
        Ok(digest(
            canonical::encode(&serde_json::to_value(self)?)?.as_bytes(),
        ))
    }

    pub fn display_name(&self) -> &str {
        self.name
            .get("zh-Hans")
            .or_else(|| self.name.get("en"))
            .map(String::as_str)
            .unwrap_or(&self.definition_id)
    }

    /// Project only onto an existing controlled role. Budgets and shell policy remain host-owned.
    pub fn project(&self, base: &crate::SubagentProfile) -> Result<crate::SubagentProfile> {
        self.validate()?;
        ensure!(base.id == self.base_role, "agent base role mismatch");
        ensure!(
            self.input_modalities == ["text"],
            "this worker adapter does not support image input"
        );
        let mut profile = base.clone();
        profile.id = self.definition_id.clone();
        profile.definition = Some(self.clone());
        profile.purpose = format!("{} [{}@{}]", self.purpose, self.version, self.revision()?);
        profile.hosted_job_prompt = false;
        profile.tool_names.retain(|tool| match tool.as_str() {
            "read_file" | "list_directory" | "search_files" | "grep_files" => {
                self.tools.iter().any(|c| c == "workspace.read")
            }
            "edit_file" | "create_file" => self.tools.iter().any(|c| c == "workspace.edit"),
            "run_command" => self.tools.iter().any(|c| c == "process.verify"),
            "git_status" | "git_diff" | "git_log" | "git_blame" => {
                self.tools.iter().any(|c| c == "repository.inspect")
            }
            _ => false,
        });
        ensure!(
            !profile.tool_names.is_empty(),
            "agent has no compatible tools"
        );
        if !self
            .tools
            .iter()
            .any(|capability| capability == "workspace.edit")
        {
            profile.write_scope = crate::subagent::SubagentWriteScope::None;
        }
        if !self
            .tools
            .iter()
            .any(|capability| capability == "process.verify")
        {
            profile.shell = crate::subagent::SubagentShell::None;
        }
        profile.capability_prompt.push_str(&format!(
            "\n\nAgent definition: {}\nRevision: {}\n{}",
            self.definition_id,
            self.revision()?,
            self.instructions
        ));
        for skill in &self.skills {
            if !skill.content.is_empty() {
                profile.capability_prompt.push_str(&format!(
                    "\n\n<skill id={:?} sha256={:?}>\n{}\n</skill>",
                    skill.id, skill.sha256, skill.content
                ));
            }
        }
        Ok(profile)
    }
}

fn bounded(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= max
}

fn unique_allowed(values: &[String], allowed: &[&str]) -> Result<()> {
    let mut seen = BTreeSet::new();
    ensure!(
        values
            .iter()
            .all(|v| allowed.contains(&v.as_str()) && seen.insert(v)),
        "unknown or duplicate capability"
    );
    Ok(())
}

impl AgentPackage {
    pub fn local(definition: AgentDefinition) -> Result<Self> {
        let sha256 = definition.revision()?;
        Ok(Self {
            definition,
            sha256,
            signature: None,
        })
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_PACKAGE_BYTES,
            "agent package is too large"
        );
        let value = canonical::parse_unique(bytes)?;
        ensure!(
            value.get("signature").is_some()
                && value
                    .get("definition")
                    .and_then(|v| v.get("forked_from"))
                    .is_some(),
            "agent package required fields missing"
        );
        let package: Self = serde_json::from_value(value)?;
        ensure!(
            package.sha256 == package.definition.revision()?,
            "agent package content hash mismatch"
        );
        Ok(package)
    }

    pub fn verify_official(&self, keys: &BTreeMap<String, String>) -> Result<()> {
        ensure!(
            self.sha256 == self.definition.revision()?,
            "agent package content hash mismatch"
        );
        let signature = self
            .signature
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("official package is unsigned"))?;
        let key = keys
            .get(&signature.key_id)
            .ok_or_else(|| anyhow::anyhow!("unknown signing key"))?;
        let key = unhex(key)?;
        let signature = unhex(&signature.value)?;
        ensure!(
            key.len() == 32 && signature.len() == 64,
            "invalid signature size"
        );
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
            .verify(
                format!("{SIGNING_DOMAIN}{}", self.sha256).as_bytes(),
                &signature,
            )
            .map_err(|_| anyhow::anyhow!("agent package signature verification failed"))
    }
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2)
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("invalid hexadecimal value");
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(Into::into))
        .collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod implementer_trial_tests;
