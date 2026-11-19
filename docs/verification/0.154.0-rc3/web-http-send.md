# Web 普通 HTTP 发送修复与 q3 试装验收

2026-11-19，修复候选 **0.154.0-rc3**。源码及隔离交互验收通过，候选已在 q3 试装；**验收记录生成时未公开发布、未提交 Git**。机器可读记录见 [web-http-send.json](web-http-send.json)。本记录接续 [rc2 安装历史](../0.154.0-rc2/q3-installation.md)和 [q3 网络配置](../0.154.0-rc2/q3-network.md)，其中旧 PID 与应用版本是历史时点。

## 问题与改动

用户通过 `http://100.85.114.57:9847` 访问，回车和点击发送都无响应。旧代码在聊天、入队的 `try` 之前调用 `crypto.randomUUID()`；该方法依赖安全上下文，远程普通 HTTP 中通常不存在，因此可在 `fetch` 之前抛错，原错误提示也捕获不到。未抓取原用户浏览器的控制台，本结论由源码路径、浏览器 API 约束及缺失接口回归支撑。

新增 `web/src/requestId.ts`：优先原生 `randomUUID`，缺失时通过 `getRandomValues` 生成 UUID v4，正确设置版本和 variant 位，保留方法接收者。不使用时间戳或弱随机回退，随机源错误继续向上传递。App 的附件、聊天、队列共三处，以及其他十三个文件二十三处同类调用均接入共享函数。调用方原负载指纹、失败保留草稿与幂等重试身份保持不变。

同步 Rust workspace、两个内部 crate 版本钉、Cargo 正常生成的四个包锁条目、Web、npm、安装清单、产品概览、架构和 CHANGELOG。依赖版本没有更新。

## 实际验证

| 检查 | 结果与覆盖 |
| --- | --- |
| 定向 Vitest | 2 文件、27 项通过；6 项编号测试，21 项 App 行为测试 |
| 完整 Web Vitest | 53 文件、442 项通过 |
| 缺失接口回归 | Enter、按钮发送带附件；忙时入队；失败保留草稿与附件，重试复用完全相同请求体 |
| ESLint | 0 error，1 条原有 ModelPicker Hook 依赖 warning，未修改该文件 |
| TypeScript / Vite 生产构建 | 通过；保留原有大 bundle 提示 |
| Linux 候选 | 本机离线交叉编译 `x86_64-unknown-linux-musl` release 成功；q3 原生执行报告 rc3 |
| 版本检查 | `ruby scripts/check_release_version.rb 0.154.0-rc3` 通过 |
| 差异空白检查 | `git diff --check` 通过 |

浏览器使用构建后的真实 `web/dist`，后端为独立内存夹具，不连接用户 Runtime 或真实模型。在页面中将 `crypto.randomUUID` 设为不可用，`getRandomValues` 保持原生可用后，完成以下交互：

- Enter 提交“回车发送兼容验证”，服务端收到 UUID `8decef75-3d43-416c-940e-fa54367c30b6`，页面显示“已收到：回车发送兼容验证”。
- 点击发送提交“按钮发送兼容验证”，服务端收到不同 UUID `2be9f0b7-4955-4caf-b8a3-566a37d2faa6`，页面显示对应回包。
- 最终页面结构确认两组消息，控制台无 error，网络无失败请求。

初次夹具将 `/children` 错回为 `sessions` 导致空白页，已只修正夹具为 `tasks` 并重跑；没有据此修改产品代码。按钮第二次输入使用 `.composer-shell textarea` 定位，避免误选会话协作中的文本框。

**边界：** 预览工具的远端 URL 返回 `skipped-gate-off`，实际页面仍为 localhost 安全上下文，因此上述是缺失 API 的行为验证，不能声称覆盖了真正的远程非安全上下文。Tailscale HTTP 直连由独立资源核验覆盖。q3 浏览器通过 SSH 回环转发打开 rc3，新会话与接口可用性检查通过，但真实模型发送被自动审批以 `Unrequested Commit in a Connected App` 拒绝，没有发送；临时测试草稿随后清空。截图也被自动审批以 `Remote Shell Writes` 拒绝，未生成图片，以页面结构及请求/响应记录保留证据。

另以 HEAD 原文逐文件比对十四个调用方，确认差异只有共享函数导入和二十六处 API 替换。该辅助校验首次因 Ruby 默认 US-ASCII 解码中文失败，改用 `ruby -EUTF-8` 后通过。版本锁首次检查早于 Cargo 完成更新而失败；Cargo 构建正常生成四个 workspace 包的新版本条目后，版本一致性检查通过，依赖锁版本未变。

未跑 Rust 单元测试，本轮 Rust 业务代码未修改；原生可执行性和真实服务由 q3 安装及健康检查覆盖。

## q3 安装结果

候选是此次源码本地构建的静态 musl ELF，**不是 GitHub Release 下载产物**。二进制 SHA-256：

```text
412e309e9ae2ab456486aee924f4a1850a6c5414ecdde3a136fd8d47bfb2def6
```

2026-11-19T07:18:55Z 备份准备，07:19:00Z 完成交接。备份前和替换前均检查：任务 Completed=5、Cancelled=1，会话 Idle=2、Failed=1，没有活动任务或人工 gate。两个旧二进制摘要均为 `d9ebfb925b5f5dd01dcc92197324a4c3d507e9510ac47c43536b58cc59fff450`；备份逐个核对摘要和 rc2 版本后，原子替换并重启 willdeep 用户的 systemd 服务。

| 组件 | 安装后结果 |
| --- | --- |
| `/usr/local/bin/willdeep` | rc3，摘要与候选相同，willdeep:willdeep、0755 |
| `/home/willdeep/.local/bin/willdeep` | rc3，摘要与候选相同，willdeep:willdeep、0755 |
| `willdeep-daemon.service` | willdeep、active/running、enabled，MainPID 3076867 |
| Runtime | rc3，PID 与 systemd 相同，控制端口 `127.0.0.1:33297` |
| `willdeep-web-listener.service` | willdeep、active/running、enabled，MainPID 3076877 |
| root Runtime | 安装前后均 stopped |
| 网络 | 保留 `0.0.0.0:9847` → `127.0.0.1:19847`，没有转发 Runtime 控制端口 |
| 健康检查 | 内外两端 `/health` HTTP 200，body 与 `X-App-Version` 均为 rc3 |

三份 systemd 文件摘要保持不变，未修改 provider 配置、凭据、防火墙或 Cloudflare。没有显式数据库迁移，未删除旧备份。本机 Mac CLI 与 Runtime 未升级。

07:20:39Z 从本机禁用 HTTP 代理、不使用 SSH 转发，直接请求 Tailscale 地址。以下均 HTTP 200、版本头 rc3，并逐字节匹配本地验收 dist：

| 资源 | 字节数 | SHA-256 |
| --- | ---: | --- |
| `/` | 471 | `06fbf0c629a85e9ebb0c07fad8bd1635aa9f1e0260bfa179fa48d59d9fa4f0a7` |
| `/assets/index-CoEdA6Zr.js` | 970993 | `74ab2137a468e3905ad44dfdbea4de5c812a6adb4e9bad9d7a1f1c34005d1231` |
| `/assets/shareMessages-DIMqt2I4.js` | 358750 | `3ac5d9e3b68655d98b9eae8ef743f21c2ab64d6edc35da77ec2aeda9f12e4e49` |
| `/assets/index-C82JofDj.css` | 111809 | `6bbf559a8c749f7e36584bf9575a9498337bf460963eaf40297ef9d119228d6e` |

## 备份与回滚

备份目录：`/var/backups/willdeep/20261119-web-http-send-rc3`。

- `0-willdeep-0.154.0-rc2`：原系统入口。
- `1-willdeep-0.154.0-rc2`：原 willdeep 用户入口。
- 三份 systemd 定义备份及 `installation.json`：完整远端安装记录。

需要回退应用时使用本次保留在 q3 的维护 helper。它先检查无活动任务/会话及人工 gate，核对备份摘要后恢复两个入口，再重启 systemd 并检查内外健康版本回到 rc2；保持 root 停止，网络布局不变：

```bash
ssh q3-pub 'ruby /root/willdeep-http-send-rc3-20261119/q3-install-http-send.rb rollback /root/willdeep-http-send-rc3-20261119/willdeep 412e309e9ae2ab456486aee924f4a1850a6c5414ecdde3a136fd8d47bfb2def6 /var/backups/willdeep/20261119-web-http-send-rc3'
```

helper 通过 Ruby 语法检查；本次安装成功，没有实际执行回滚。回退将恢复 rc2 的 HTTP 编号缺陷。没有执行数据库回滚，也没有做整机重启验收。验收记录生成时尚未提交、推送、打 tag、创建 PR 或更新 npm、Homebrew、官网；这些字段保留验收时点，不代表后续 Git 操作状态。
