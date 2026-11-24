# LANLink

LANLink 是面向 macOS 和 Windows 的开源局域网聊天与文件传输工具。在同一局域网内，电脑之间可以聊天、互传文件，手机和平板也可以扫码与电脑双向收发文字和文件。使用时无需访问公网或注册账号。

[官网首页](https://lanlink.heyyami.com/) · [下载安装包](https://lanlink.heyyami.com/#download) · [安装指南](docs/install.md) · [GitHub 发行记录](https://github.com/JackEngineer/LanLink/releases)

Mac 与 Windows 已通过配对、双向聊天、双向 1 GiB 文件及断线恢复实测。[验收报告](docs/acceptance.md)

Mac 默认打开独立桌面窗口，Windows 通过本机浏览器使用。手机和电脑连同一个可信 Wi‑Fi，电脑确认后即可双向收发文字和文件，不用安装移动端 App。[扫码使用说明](docs/install.md#与手机或平板扫码传文件)

## 下载 v0.2.1 正式版

要让 AI 助手安装，可以复制下面这句话，粘贴给能操作这台电脑的助手。助手会先检查已有安装，再按需下载与你的系统匹配的正式版，校验 SHA-256 后安装并打开 LANLink。[查看完整安装步骤](apps/site/install-prompt.txt)

```text
请按 https://lanlink.heyyami.com/install.txt 安装并打开 LANLink。
```

从 [LANLink 官网](https://lanlink.heyyami.com/#download) 选择与你的电脑匹配的正式版安装包。

| 平台                        | 下载包                                                                                                                                                                                   |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Apple Silicon Mac（M 系列） | [ZIP](https://lanlink.heyyami.com/downloads/v0.2.1/LANLink-0.2.1-aarch64-apple-darwin.zip) 或 [PKG](https://lanlink.heyyami.com/downloads/v0.2.1/LANLink-0.2.1-aarch64-apple-darwin.pkg) |
| Intel Mac                   | [ZIP](https://lanlink.heyyami.com/downloads/v0.2.1/LANLink-0.2.1-x86_64-apple-darwin.zip)                                                                                                |
| Windows x64                 | [ZIP](https://lanlink.heyyami.com/downloads/v0.2.1/LANLink-0.2.1-x86_64-pc-windows-msvc.zip)                                                                                             |

macOS 包未通过 Apple 签名或公证，Windows 包仍通过本机浏览器打开。下载后按[安装指南](docs/install.md)操作。安装包校验值见 [SHA256SUMS](https://lanlink.heyyami.com/downloads/v0.2.1/SHA256SUMS)。

已安装 `v0.2.0-preview.4` 或 `v0.2.0-preview.5` 的用户可以在「设置」点击「一键升级」。从官网下载的 `v0.2.0-preview.5` 已在 Mac mini 和 Windows x64 上实测升级到正式版。[一键升级验收](docs/acceptance.md#v020-一键升级实机验收)

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

Intel Mac 构建先安装对应 target，再执行 `bash scripts/macos/package.sh x86_64-apple-darwin`。交叉构建不代替 Intel 实机验证。默认开发包使用 ad-hoc 签名。构建可分发的正式版需要 Developer ID 签名和 Apple 公证，配置见[发布手册](deploy/release-sync/README.md#macos-正式版签名与公证)。

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
