"""Harbor ``BaseInstalledAgent`` wrapper around ``willdeep run``."""

from __future__ import annotations

import re
import shlex
from typing import Annotated, Literal, override

from pydantic import Field

from harbor.agents.agent_auth_resolution import (
    ModelConnectionSpec,
    ProviderAccess,
    require_model_api_key,
)
from harbor.agents.capabilities import AgentCapabilities
from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.agents.options import Cli, InstalledAgentOptions
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext
from harbor.models.model_api_format import ModelApiFormat

# Last tag with linux-amd64 on GitHub Releases (bump after Release workflow ships).
_DEFAULT_RELEASE = "v0.154.0-rc11"
_DEFAULT_LINUX_ASSET = (
    "https://github.com/niuwoai/willdeep-cli/releases/download/"
    f"{_DEFAULT_RELEASE}/willdeep-linux-amd64.tar.gz"
)
_REMOTE_WILLDEEP_HOME = "/tmp/willdeep-harbor"
_AGENT_LOG = "/logs/agent/willdeep.txt"
_PROMPT_PATH = "/tmp/harbor-willdeep-prompt.txt"
_DEFAULT_WORKDIR = "/app"

_SNAPSHOT_UNTRACKED_WORKSPACE = (
    "if command -v git >/dev/null 2>&1 "
    "&& ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then "
    "{ git init -q && timeout 120 git add -A "
    "&& git -c user.name=Agent -c user.email=agent@localhost "
    "commit -q --no-verify --allow-empty -m 'Task files before agent run'; } "
    "|| echo 'agent: could not snapshot the workspace for willdeep' >&2; "
    "fi; "
)


class WilldeepOptions(InstalledAgentOptions):
    version: Annotated[str | None, Cli("--version")] = Field(
        default=_DEFAULT_RELEASE,
        description=(
            "GitHub release tag used to download willdeep-linux-amd64.tar.gz "
            "(for example v0.155.0-rc6)."
        ),
    )
    install_url: Annotated[str | None, Cli("--install-url")] = Field(
        default=None,
        description="Override the linux-amd64 tarball URL; --version is ignored when set.",
    )
    provider: Annotated[
        Literal["some-im", "openai-compatible", "anthropic", "auto"],
        Cli("--provider"),
    ] = Field(
        default="some-im",
        description="Provider identity written into the generated config.toml.",
    )
    profile: Annotated[str, Cli("--profile")] = Field(
        default="harbor",
        description="Provider profile name in the generated config.toml.",
    )
    max_turns: Annotated[int | None, Cli("--max-turns")] = Field(
        default=80,
        ge=1,
        le=500,
        description="Maximum model/tool rounds for willdeep run.",
    )
    local_harness: Annotated[bool, Cli("--local-harness")] = Field(
        default=True,
        description="Pass --local to use the in-process harness (recommended in Docker).",
    )
    full_auto: Annotated[bool, Cli("--full-auto")] = Field(
        default=True,
        description="Pass --full-auto so workspace edits do not block on approval.",
    )
    language: Annotated[Literal["en", "zh-CN", "ja"], Cli("--language")] = Field(
        default="en",
        description="UI/system language for the agent inside the sandbox.",
    )
    workdir: Annotated[str, Cli("--workdir")] = Field(
        default=_DEFAULT_WORKDIR,
        description="Workspace root passed to willdeep run -w.",
    )


class WilldeepAgent(BaseInstalledAgent):
    """Install willdeep-cli into the task container and run one headless episode."""

    capabilities = AgentCapabilities(base_url=True, resume=False)
    MODEL_CONNECTION = ModelConnectionSpec(
        agent_provider_access=ProviderAccess(
            api_key_candidate_names=["SOMEIM_API_KEY", "OPENAI_API_KEY"],
            base_url_candidate_names=[
                "OPENAI_BASE_URL",
                "OPENAI_API_BASE",
                "WILLDEEP_API_BASE",
            ],
        ),
        supported_api_formats=[
            ModelApiFormat.OPENAI_CHAT_COMPLETIONS,
            ModelApiFormat.ANTHROPIC_MESSAGES,
        ],
        base_url_destination_names={
            ModelApiFormat.OPENAI_CHAT_COMPLETIONS: "WILLDEEP_API_BASE",
        },
    )
    options_model = WilldeepOptions
    options: WilldeepOptions

    @staticmethod
    @override
    def name() -> str:
        return "willdeep"

    @classmethod
    @override
    def check_model_credentials(cls, connection, *, kwargs, env, explicit_env=None):
        require_model_api_key(connection, cls.name())

    @override
    def get_version_command(self) -> str | None:
        return 'export PATH="$HOME/.local/bin:$PATH"; willdeep --version'

    @override
    def parse_version(self, stdout: str) -> str:
        match = re.search(r"(\d+\.\d+\.\d+(?:-rc\d+)?)", stdout)
        return match.group(1) if match else stdout.strip()

    def _linux_install_url(self) -> str:
        if self.options.install_url:
            return self.options.install_url
        tag = self.options.version or self._version or _DEFAULT_RELEASE
        if tag.startswith("http://") or tag.startswith("https://"):
            return tag
        return (
            "https://github.com/niuwoai/willdeep-cli/releases/download/"
            f"{tag}/willdeep-linux-amd64.tar.gz"
        )

    def _model_id(self) -> str:
        if not self.model_name:
            raise ValueError("--model is required (for example glm-5 or some-im/glm-5)")
        if "/" in self.model_name and not self.skip_model_parsing:
            return self.model_name.split("/", 1)[1]
        return self.model_name

    def _api_wire(self) -> str:
        if self._model_api_format == ModelApiFormat.ANTHROPIC_MESSAGES:
            return "anthropic-messages"
        return "chat-completions"

    @override
    async def install(self, environment: BaseEnvironment) -> None:
        await self.ensure_system_dependencies(
            environment, ("curl", "tar", "git", "ca_certificates", "coreutils")
        )
        url = shlex.quote(self._linux_install_url())
        await self.exec_as_agent(
            environment,
            command=(
                "set -euo pipefail; "
                "mkdir -p \"$HOME/.local/bin\"; "
                f"curl -fsSL {url} | tar -xz -C \"$HOME/.local/bin\"; "
                "chmod +x \"$HOME/.local/bin/willdeep\"; "
                'export PATH="$HOME/.local/bin:$PATH"; willdeep --version'
            ),
        )

    @override
    async def configure(self, environment: BaseEnvironment) -> None:
        access = self.model_connection
        api_base = access.execution_base_url or "https://some.im/v1"
        key_env = self._model_api_key_name or "SOMEIM_API_KEY"
        model = self._model_id()
        profile = self.options.profile
        provider = self.options.provider
        api = self._api_wire()
        max_turns = self.options.max_turns or 80
        language = self.options.language

        config = (
            "version = 1\n"
            f'default_provider = "{profile}"\n\n'
            "[agent]\n"
            f"max_turns = {max_turns}\n"
            'approval = "smart"\n'
            f'language = "{language}"\n'
            "small_model_routing = false\n"
            "auto_title = false\n\n"
            f"[providers.{profile}]\n"
            f'provider = "{provider}"\n'
            f'api = "{api}"\n'
            f'api_base = "{api_base}"\n'
            f'api_key_env = "{key_env}"\n'
            f'model = "{model}"\n'
        )

        await self.exec_as_agent(
            environment,
            command=(
                f"mkdir -p {shlex.quote(_REMOTE_WILLDEEP_HOME)} && "
                f"cat > {shlex.quote(_REMOTE_WILLDEEP_HOME)}/config.toml << 'WILLDEEPEOF'\n"
                f"{config}"
                "WILLDEEPEOF"
            ),
        )

    @override
    def populate_context_post_run(self, context: AgentContext) -> None:
        return

    @override
    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        access = self.model_connection
        env = {**access.env, **self.resolve_env_vars()}
        env["WILLDEEP_HOME"] = _REMOTE_WILLDEEP_HOME
        if access.api_key is not None and self._model_api_key_name:
            env[self._model_api_key_name] = access.api_key
        if access.execution_base_url:
            env["WILLDEEP_API_BASE"] = access.execution_base_url

        model = shlex.quote(self._model_id())
        config_path = shlex.quote(f"{_REMOTE_WILLDEEP_HOME}/config.toml")
        workdir = shlex.quote(self.options.workdir)
        flags: list[str] = ["--output", "json"]
        if self.options.local_harness:
            flags.append("--local")
        if self.options.full_auto:
            flags.append("--full-auto")
        if self.options.max_turns:
            flags.extend(["--max-turns", str(self.options.max_turns)])
        extra = " ".join(shlex.quote(part) for part in flags)
        profile = shlex.quote(self.options.profile)

        await self.exec_as_agent(
            environment,
            command=(
                f"cat > {shlex.quote(_PROMPT_PATH)} << 'PROMPT_EOF'\n"
                f"{instruction}\n"
                "PROMPT_EOF"
            ),
            env=env,
            timeout_sec=30,
        )

        await self.exec_as_agent(
            environment,
            command=(
                f"{_SNAPSHOT_UNTRACKED_WORKSPACE}"
                'export PATH="$HOME/.local/bin:$PATH"; '
                f"{extra} willdeep run "
                f"-p {profile} "
                f"--config {config_path} "
                f"-m {model} "
                f"-w {workdir} "
                f"--input {shlex.quote(_PROMPT_PATH)} "
                f"2>&1 | stdbuf -oL tee {shlex.quote(_AGENT_LOG)}"
            ),
            env=env,
        )

