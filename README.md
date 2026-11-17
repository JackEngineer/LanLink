# LANLink

LANLink 是面向 macOS 和 Windows 的开源局域网聊天与文件传输工具。每台设备运行 Rust Agent，浏览器访问本机 React 界面。运行时不需要公网服务或第三方账号。

当前为 v0.1.0 开发版本。功能和平台验收状态见 [验收报告](docs/acceptance.md)。未完成双平台实测前不发布正式完成声明。

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

双 Agent 集成脚本会启动隔离的数据目录，不使用已有用户数据。其执行方式和实际结果将记录在验收报告。

## 构建安装包

在 Apple Silicon Mac 上执行以下命令。

```sh
bash scripts/macos/package.sh
```

Intel Mac 构建先安装对应 target，再执行 `bash scripts/macos/package.sh x86_64-apple-darwin`。交叉构建不代替 Intel 实机验证。macOS 产物使用本地 ad-hoc 签名，尚未提供 Developer ID 签名或 Apple 公证。

在 Windows x64 开发者环境执行 `./scripts/windows/package.ps1`。ZIP 内 `Install.ps1` 将程序安装到当前用户目录。使用 `-StartAtLogin` 选项开启登录启动。防火墙提示仅允许受信任的专用网络。

安装、启动和卸载说明见 [安装指南](docs/install.md)。

## 工程结构

- `apps/web` 保存 React、TypeScript、Tailwind、Zustand 和 TanStack Query 前端。
- `crates/protocol` 定义共享协议和数据模型。
- `crates/store` 管理 SQLite 持久化。
- `crates/transport` 管理 Noise 加密和帧边界。
- `crates/agent` 运行 HTTP、WebSocket、mDNS 和业务任务。
- `scripts` 保存本地检查、打包和集成验收工具。

[架构文档](docs/architecture.md)解释设计选择。[本机 API](docs/api-contract.md)定义前后端契约。[需求与完成门槛](docs/brief.md)定义验收范围。

## 开源许可

MIT。见 [LICENSE](LICENSE)。
