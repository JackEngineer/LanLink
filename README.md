# LANLink

LANLink 是面向 macOS 和 Windows 的开源局域网聊天与文件传输工具。每台设备运行 Rust Agent，浏览器访问本机 React 界面。运行时不需要公网服务或第三方账号。

Mac 与 Windows 已通过配对、双向聊天、双向 1 GiB 文件及断线恢复实测。[验收报告](docs/acceptance.md)

从源码构建的版本提供手机和平板扫码传文件。手机和电脑连同一个可信 Wi‑Fi，电脑确认连接后即可双向收发，不用安装移动端 App。[扫码使用说明](docs/install.md#与手机或平板扫码传文件)

## 下载开发预览

从 [GitHub Release v0.2.0-preview.1](https://github.com/JackEngineer/LanLink/releases/tag/v0.2.0-preview.1) 选择与你的电脑匹配的安装包。

| 平台                        | 下载包                                                                                                                                                                                                                                               |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Apple Silicon Mac（M 系列） | [ZIP](https://github.com/JackEngineer/LanLink/releases/download/v0.2.0-preview.1/LANLink-0.2.0-aarch64-apple-darwin.zip) 或 [PKG](https://github.com/JackEngineer/LanLink/releases/download/v0.2.0-preview.1/LANLink-0.2.0-aarch64-apple-darwin.pkg) |
| Intel Mac                   | [ZIP](https://github.com/JackEngineer/LanLink/releases/download/v0.2.0-preview.1/LANLink-0.2.0-x86_64-apple-darwin.zip)                                                                                                                              |
| Windows x64                 | [ZIP](https://github.com/JackEngineer/LanLink/releases/download/v0.2.0-preview.1/LANLink-0.2.0-x86_64-pc-windows-msvc.zip)                                                                                                                           |

下载后按[安装指南](docs/install.md)操作。安装包校验值见 [SHA256SUMS](https://github.com/JackEngineer/LanLink/releases/download/v0.2.0-preview.1/SHA256SUMS)。

## 开发运行

需要 Node.js 24、npm 和 Rust stable。首次下载构建依赖需要网络，编译后运行无需访问公网。

```sh
npm ci
npm run build
cargo run -p lanlink-agent -- --web-dir apps/web/dist
```

打开 `http://127.0.0.1:38761`。HTTP 只监听本机回环地址。局域网 TCP 默认端口为 38762，端口冲突后通过 mDNS 广播实际监听端口。

## 验证

```sh
npm run lint
npm test
npm run build
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

双 Agent 集成脚本会启动隔离的数据目录，不使用已有用户数据。执行方式和实际结果见 [验收报告](docs/acceptance.md#可复现集成验证)。

构建网页和 Agent 后，运行 `python3 scripts/mobile-smoke.py --agent target/debug/lanlink-agent --web-dir apps/web/dist`，可用本机局域网 IPv4 验证扫码会话的授权、双向文件和撤销。

## 构建安装包

在 Apple Silicon Mac 上执行以下命令。

```sh
bash scripts/macos/package.sh
```

Intel Mac 构建先安装对应 target，再执行 `bash scripts/macos/package.sh x86_64-apple-darwin`。交叉构建不代替 Intel 实机验证。macOS 产物使用本地 ad-hoc 签名，尚未提供 Developer ID 签名或 Apple 公证。

在 Windows x64 开发者环境执行 `./scripts/windows/package.ps1`。ZIP 内 `Install.ps1` 将程序安装到当前用户目录。使用 `-StartAtLogin` 选项开启登录启动。防火墙提示仅允许受信任的专用网络。

安装、启动和卸载说明见 [安装指南](docs/install.md)。

## 工程结构

- `apps/site` 保存独立官网，运行 `npm run site:package` 生成静态发布包。腾讯云 Nginx 部署见[官网部署指南](docs/website-deploy.md)。
- `apps/web` 保存 React、TypeScript、Tailwind、Zustand 和 TanStack Query 前端。
- `crates/protocol` 定义共享协议和数据模型。
- `crates/store` 管理 SQLite 持久化。
- `crates/transport` 管理 Noise 加密和帧边界。
- `crates/agent` 运行 HTTP、WebSocket、mDNS 和业务任务。
- `scripts` 保存本地检查、打包和集成验收工具。

[架构文档](docs/architecture.md)解释设计选择。[本机 API](docs/api-contract.md)定义前后端契约。[需求与完成门槛](docs/brief.md)定义验收范围。

## 开源许可

MIT。见 [LICENSE](LICENSE)。
