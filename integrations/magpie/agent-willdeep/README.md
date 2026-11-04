# WillDeep Desktop · Magpie Agent

本地可安装插件，版本 **0.1.0-rc2**，Agent ID `willdeep`。npm 包标识 `willdeep-magpie-agent-desktop` 仅作本地元数据；尚未发布或被社区收录。预计最低客户端版本 **WillDeep Desktop 1.427.0-rc1**；客户端整体调用验收由双端集成测试负责。

## 安装和连接

1. 安装支持版本的 WillDeep Desktop，并至少启动一次，使 App 创建 `~/.willdeep/integrations/magpie/desktop/client.json` 和私有目录。
2. 保持本包文件在同一目录，用 Magpie 的 Plugins 页面添加该本地目录，或执行 `magpie plugin add /绝对路径/agent-willdeep`。无需运行 `yarn install` 或构建。
3. 启用插件，在 Agents → WillDeep 选择 Magpie 中的模型。完整模型 ID 由宿主写入 `model`，包括供应商或路由前缀。
4. 在 WillDeep 创建没有显式 Provider/模型选择的新会话。连接与切模对新会话生效，已有会话保留其 Provider/模型。

描述符只用 Desktop 私有目录检测“曾启动支持版本”，不检测共享 `.willdeep` 根目录，不依赖 `willdeep-app`。残留目录不能证明 App 仍已安装或版本已达标；插件不能执行客户端版本检查。

## 断开和边界

在 Magpie 将 WillDeep 恢复默认/断开，或执行 `magpie willdeep default`。Magpie 恢复自己托管的 JSON 字段；新会话恢复客户端原默认。原先明确绑定集成的会话下一次执行应提示重新连接或改选 Provider，不应悄悄转发到其他供应商。正在执行的任务不因断开取消。

本包唯一配置目标是 `~/.willdeep/integrations/magpie/desktop/connection.json`，不写 UserDefaults Provider 库、钥匙串或 CLI TOML。该 JSON 由 Magpie 管理，不适合混入用户自定义字段。断开可留下 `{}` 或仅含合法 `model` 的对象；后者仍表示未连接，不能用作客户端默认模型。

只支持本机 `http://` / `https://` 回环网关，地址精确以 `/v1` 结尾。`gateway.key` 必须精确为固定公开标记 `magpie-willdeep`；真实网关 Key 和远端/共享网关首版拒绝。未知或非法窗口/输出元数据写 `null`。插件不执行文件、网络、进程或 Node API，写入由宿主完成。

## 测试与兼容

本包原生测试：在此目录运行 `yarn test`。全量边界测试和 Ruby 真实宿主验证见父目录 README。描述符按 Magpie 固定源码 `759b4d1d7da8b983860539e3bd8ce1eaf70a06a4` 文档核对；真实宿主加载/设模/恢复已在 Magpie **0.1.2891** 隔离 HOME 验证。该证据不等于完整 Desktop 推理验收。

图标以包内 `icon.svg` 供审查，运行时 `icon.js` 使用对应 data URL，不请求外部图片。提示与错误在 `messages.js` 中提供中英文资源；宿主 `notice` 是单字符串，因此显示双语。

## English

Local package **0.1.0-rc2**, agent ID `willdeep`. Expected minimum client: **WillDeep Desktop 1.427.0-rc1**. This package is not published and has not been admitted to the community registry.

Launch a supported Desktop version once, then add this directory with `magpie plugin add /absolute/path/agent-willdeep` or the Plugins page. No dependency installation or build is needed. Enable it and select a Magpie model under Agents → WillDeep. Only new sessions without an explicit choice adopt the connection; existing sessions retain their Provider and model.

The App-created private Desktop support directory is the installation signal, not the shared `.willdeep` directory or an optional launcher. A stale directory cannot prove an installed or sufficiently new App. The plugin does not execute version checks.

Disconnect in Magpie or run `magpie willdeep default`. Magpie restores its managed fields; an empty object or a native `model` alone is disconnected. Explicitly integrated sessions must ask for reconnection or another Provider when continued. Original Provider preferences, Keychain and CLI TOML are not edited.

Only loopback HTTP(S), exact `/v1`, and the public `magpie-willdeep` marker are supported. Real gateway keys and remote/shared gateways are rejected. Unknown metadata becomes `null`. The production module has no Node, filesystem, network or process access.

Run `yarn test` here; see the parent README for complete native tests and the Ruby real-host check. Real plugin loading, model selection and restoration passed on isolated Magpie **0.1.2891**. Desktop inference/session behavior requires the separate consumer E2E tests.
