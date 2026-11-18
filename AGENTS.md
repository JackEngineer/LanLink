# LANLink 仓库协作指南

LANLink 面向 macOS 和 Windows。桌面设备运行 Rust Agent，通过本机浏览器收发消息和文件。手机和平板通过局域网扫码传文件。应用运行时应保持离线可用。

## 代码入口

| 路径 | 职责 |
| --- | --- |
| `apps/web` | Agent 提供的 React、TypeScript 和 Vite 界面 |
| `apps/site` | 独立静态官网及下载入口 |
| `crates/agent` | 本机 HTTP、WebSocket、mDNS、扫码连接及业务流程 |
| `crates/protocol`、`crates/transport` | 共享协议、Noise 会话和传输分帧 |
| `crates/store` | SQLite 持久化 |
| `scripts`、`deploy` | 本地检查、平台打包、集成验收和官网部署 |

## 开发约定

- 修改本机 API 或互传协议时，同步核对调用方和对应契约。入口见[本机 API](docs/api-contract.md)与[传输协议](docs/protocol.md)。
- 保持本机管理 HTTP 的回环绑定及 Host、Origin 和会话令牌检查。手机扫码使用独立的局域网监听及授权流程。涉及配对、信任或文件发布时，核对[架构约束](docs/architecture.md)。
- 修改消息或文件持久化时，验证重启恢复。文件续传以接收方已持久化的偏移为准，完成前校验 SHA-256，保存时避免覆盖已有文件。

## 本地验证

使用 Node.js 24、npm 和 Rust stable。在仓库根目录运行对应检查。

| 改动范围 | 验证入口 |
| --- | --- |
| Agent、协议、存储或应用前端 | `bash scripts/check.sh`，包含前端检查、构建和测试，以及 Rust 格式、Clippy 和测试 |
| 独立官网 | `npm run site:check`，验证部署产物时使用 `npm run site:package` |
| 官网安装包同步 | `python3 -B -m unittest discover -s deploy/release-sync` |
| 桌面互传与断线恢复 | 按[验收记录](docs/acceptance.md#可复现集成验证)运行 `scripts/smoke.py` |
| 手机扫码和记住浏览器 | 构建 Agent 与网页后，使用 `scripts/mobile-smoke.py` 和 `scripts/mobile-remember-smoke.py`，参见[扫码验收记录](docs/acceptance.md#扫码传文件) |

集成验证使用隔离数据目录。安装、登录启动和卸载在对应原生平台验证，真实局域网互通参照[物理双机验收](docs/physical-final.md)。在 PR 中写明实际验证的平台、候选提交或安装包，以及未执行的项目。

## 官网与安装包发布

- PR 检查由 [Build and test](.github/workflows/build.yml) 定义。推送发行标签会触发[构建、打包和签名工作流](.github/workflows/release.yml)。发布时将该次运行产出的四个安装包、校验清单及签名证明原样上传至对应 GitHub Release。
- 官网同步要求验证标签、源码提交、审核过的工作流、签名证明和安装包摘要。更新工作流时，按[自动发布手册](deploy/release-sync/README.md)同步处理服务器上的审核副本。发布或回滚时保留历史下载直链。
- 手动部署按[官网部署指南](docs/website-deploy.md)执行。完成后核查公网首页、校验清单和安装包，记录实际在线版本及结果。验收示例见[官网验收记录](docs/website-acceptance.md)。
