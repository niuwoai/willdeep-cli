# WillDeep CLI · Magpie Agent

本地可安装插件，版本 **0.1.0-rc2**，Agent ID `willdeep-cli`。npm 包标识 `willdeep-magpie-agent-cli` 仅作本地元数据；尚未发布或被社区收录。预计最低客户端版本 **WillDeep CLI 0.142.0-rc1**，可执行命令是 **`willdeep`**。

## 安装和连接

1. 安装支持版本的 CLI，确保 Magpie 所在进程的 PATH 可以找到 `willdeep`。
2. 保持本包文件在同一目录，用 Plugins 页面添加本地目录，或执行 `magpie plugin add /绝对路径/agent-willdeep-cli`。无需运行 `yarn install` 或构建。
3. 启用插件，在 Agents → WillDeep CLI 选择 Magpie 模型。宿主将完整 `model.id` 写入 `model`，不移除供应商或路由前缀。
4. 用 CLI、TUI 或 Runtime 创建没有显式选择的新会话。已有会话保留其 Profile/模型，显式其他 Profile/模型、审批/沙箱和显式 Worker 设置沿用客户端规则。

默认配置目标为 `~/.willdeep/integrations/magpie/cli/connection.json`。描述符声明 `bin: willdeep`，不把 Desktop 私有支持目录或共享 `.willdeep` 根目录作为 CLI 检测依据。宿主还可能根据本 Agent 的既有配置文件进行检测；这不等同于执行客户端版本检查。

## 自定义 WILLDEEP_HOME

发行描述符只指向标准目录。给 CLI 设置 `WILLDEEP_HOME` 不会自动改变已运行的 Magpie 环境或插件路径。

为自定义根目录复制整个本地插件包，在副本的 `willdeep-cli.agent.js` 中将 `config` 改为该根目录下的绝对路径，例如 `/你的目录/integrations/magpie/cli/connection.json`。也可写 `$WILLDEEP_HOME/integrations/magpie/cli/connection.json`，前提是 **Magpie 进程本身**同样设置了该变量。宿主仅展开 `~` 和 `$VAR`，不支持 `${VAR:-default}`。

先在 Magpie 断开标准目录的条目，再移除原插件并添加副本；两个同 ID 插件不能同时启用。CLI 使用同一个自定义根目录，且不得回退读取标准 Home 的连接。更换路径前先断开，可避免旧路径继续保留有效连接。

## 断开和安全边界

在 Magpie 将 WillDeep CLI 恢复默认/断开，或执行 `magpie willdeep-cli default`。Magpie 恢复自己托管的 JSON 字段，新会话恢复客户端原默认。原来显式绑定 `magpie-cli` 的会话续聊应提示重连或改选，不转发到其他供应商；正在执行的任务不因断开取消。

不修改 CLI TOML、Desktop Provider 库或钥匙串。`connection.json` 由 Magpie 托管，不适合混入用户字段。断开可留下 `{}` 或仅有合法 `model` 的对象；仅模型不成为外部默认配置。

只接受本机小写 HTTP(S)、规范回环地址和精确 `/v1` 路径。`gateway.key` 必须精确为固定公开标记 `magpie-willdeep-cli`，真实网关 Key、远端/共享网关首版拒绝。未知或非法元数据为 `null`；插件不编造窗口或输出上限。生产 JS 不调用 Node、文件、网络或进程 API。

## 测试与兼容

在此目录运行 `yarn test`。全量边界测试、共享 fixture 投影和 Ruby 真实宿主校验见父目录 README。按 Magpie 固定源码 `759b4d1d7da8b983860539e3bd8ce1eaf70a06a4` 的文档核对描述符；加载、设模、断开、自定义根目录已在隔离 HOME 的 Magpie **0.1.2891** 验证。CLI/Runtime 真正推理和会话固定需由消费端 E2E 验收。

`icon.svg` 与运行时 data URL 对应，不加载外部图标。`messages.js` 提供中英文提示和错误；由于宿主只有一个 `notice` 字符串，提示显示双语。

## English

Local package **0.1.0-rc2**, agent ID `willdeep-cli`, expected minimum client **WillDeep CLI 0.142.0-rc1**, executable **`willdeep`**. Unpublished; not yet in the community registry.

Ensure Magpie can find `willdeep` on its PATH. Add this directory through Plugins or `magpie plugin add /absolute/path/agent-willdeep-cli`, enable it and select a model under Agents → WillDeep CLI. No build or dependency installation is needed. Only new sessions without an explicit choice use the connection; existing session selections remain fixed.

The default target is `~/.willdeep/integrations/magpie/cli/connection.json`. Desktop's directory is not a CLI detection signal. Magpie may also detect this Agent's own existing config; detection does not prove the client version.

For a custom `WILLDEEP_HOME`, copy the entire local package and edit its descriptor `config` to the absolute custom path. `$WILLDEEP_HOME/integrations/magpie/cli/connection.json` also works if Magpie itself has that variable. A CLI environment change does not update a running Magpie. Disconnect the old target before removing the original plugin and adding the copy; do not enable two packages with the same Agent ID.

Disconnect in Magpie or run `magpie willdeep-cli default`. Magpie restores its managed fields. A model-only object is disconnected and never supplies a default model. Existing explicitly integrated sessions must ask for reconnection or a different Profile when continued. Original TOML and Desktop preferences/Keychain remain unchanged.

Only loopback HTTP(S), exact `/v1` and the public `magpie-willdeep-cli` marker are supported; real keys and remote/shared gateways are rejected. Unknown metadata is `null`. Production modules use no Node, filesystem, networking or process APIs.

Run `yarn test` here; see the parent README for full tests and the Ruby real-host check. Magpie **0.1.2891** passed isolated loading, model writing, restoration and custom-home checks. Actual CLI/Runtime inference and session persistence require consumer E2E tests.
