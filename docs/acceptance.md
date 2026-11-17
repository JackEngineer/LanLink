# v0.1.0 验收记录

v0.1.0 尚未完成。Mac 本机双 Agent 的核心流程已实测，Windows 安装及两台物理电脑互通仍是发布门槛。

## 环境与候选身份

2026-11-17 在 Apple Silicon macOS 27.0 上执行。Rust 1.100.0，Node.js 24.22.2。最初的大文件发布构建来自 `4c6aeb7efc66c38dc853e08f24ee998103ad626c`，二进制 SHA-256 为 `f76003b4a24a6dbf61d19531686b6d41b5d921291621933502fe67cd7fef3bf9`。

最终安装包内 Agent 的 SHA-256 为 `383d4358389f08a6a4b55386c4b58db0510410528489f32d70db4cc3b59106ba`，对应源码提交 `a15025c0407bd668fa3b1378393964f1984645b0`，含上述提交加独立复核过的 crates 差异，差异 SHA-256 为 `bd73f8dd9e7d6ed3af17530b58794e9f38b33b500994575f6aeb8cb0ad893e4b`。[最终完整集成日志](evidence/mac-release-final.log)在该打包二进制上通过全部选定检查。

[Rust 27 项测试](evidence/rust-tests.log)、[严格 Clippy](evidence/clippy.log)和 `cargo fmt --all -- --check` 均通过。

[二进制身份与命令](evidence/artifact-binding.txt)、[直接记录产物哈希的再次完整验证](evidence/mac-release-hash-bound.log)、[原生安装生命周期命令输出](evidence/mac-native.log)及[前端测试输出](evidence/web-tests.log)分别保留对应证据。公开日志中的用户名路径和私网 IP 已脱敏，原始日志保留在开发机；临时设备 ID 来自隔离测试身份。

前端生产构建、ESLint 和 13 项 Vitest 测试通过。升级测试依赖后 `npm audit` 报告 0 项漏洞。以上是工具运行结果，不构成无安全缺陷保证。

## 测试结果

| 编号 | 内容               | Mac 本机双 Agent                               | Mac 与 Windows 双机 |
| ---- | ------------------ | ---------------------------------------------- | ------------------- |
| T01  | Mac 启动 Agent     | 通过                                           | Mac 已验证          |
| T02  | Windows 启动 Agent | 不适用                                         | 待验证              |
| T03  | 自动发现           | 真实 mDNS 通过                                 | 待验证              |
| T04  | 离线状态更新       | 正常停止与重启通过；异常断电广播过期未独立计时 | 待验证              |
| T05  | 上线自动恢复       | 通过                                           | 待验证              |
| T06  | 未配对发送拒绝     | 通过                                           | 待验证              |
| T07  | 双向文本通信       | API 与浏览器均通过                             | 待验证              |
| T08  | 历史消息恢复       | 通过                                           | 待验证              |
| T09  | 10 MiB 文件        | 磁盘与 HTTP 下载哈希均一致                     | 待验证              |
| T10  | 双向 1 GiB 文件    | 通过                                           | 待验证              |
| T11  | 传输中断恢复       | 1 GiB 自动续传通过，首发偏移日志吻合           | 待验证              |
| T12  | SHA-256 一致       | 通过                                           | 待验证              |
| T13  | 身份与数据保留     | 通过                                           | 待验证              |
| T14  | 撤销授权           | 拒绝本地发送，观察 5 秒无新接收                | 待验证              |
| T15  | IP 改变重新发现    | 未执行                                         | 待验证              |

Mac 本机双进程集成测试不代替物理双机、防火墙及跨平台文件系统测试。内存、启动、发现延迟和 ACK 性能目标尚未测量。

## 可复现集成验证

```sh
cargo build --release --locked -p lanlink-agent
npm run build
python3 scripts/smoke.py --agent target/release/lanlink-agent --web-dir apps/web/dist --file-mib 1024 --resume-mib 1024 --file-timeout 300
```

脚本使用隔离的数据目录、随机端口和实际 Agent 进程。没有模拟设备或传输。它在结束时停止自己启动的进程，保留目录与日志供检查。日志中的 mDNS 地址包含本机 LAN 地址和回环地址，不能据此声称跨物理设备传输。

[初次 10 MiB 与 64 MiB 验证](evidence/mac-smoke-initial.log)覆盖持久化、授权拒绝、重试、取消和撤销。[发布构建 1 GiB 验证](evidence/mac-release-1g-initial.log)增加反向传输和真实续传起点检查。双向完整文件的 SHA-256 为 `dd921821c346ba3ca8bb0e60659a0c60dd45bd4d0fa9a875e96d032707f824a0`。中断测试从持久化的 2 MiB 偏移恢复，最终哈希为 `7e37e2e55ea3675ea0fd50ba2627f9bb568b2e11257937c9064e26f4ea147c0d`。

## 浏览器证据

真实双 Agent 网页完成双方核对安全码并分别批准、双向聊天、选择文件、接收端接受、传输完成、设备改名、390px 窄屏导航。控制台未见警告或错误。上传后进入传输记录的改动已在浏览器验证，混合超限文件批次保留聊天页错误提示。

![双向聊天](evidence/chat-desktop.jpg)
![窄屏聊天](evidence/chat-mobile.jpg)
![窄屏接收文件](evidence/transfer-mobile.jpg)
![上传后显示待接收状态](evidence/upload-feedback.jpg)
![混合文件批次保留超限提示](evidence/mixed-upload-limit.jpg)

## 首次失败与修复

最初 10 MiB 上传失败，原因是读取下一个 multipart 字段前仍持有前一个字段。释放该字段后重新执行全部选定流程通过。早期 debug 1 GiB 测试被主动停止，因未明确启用续传偏移日志且不代表发行构建；未将该轮计为通过。

[快速重启发现失败记录](evidence/mac-smoke-mdns-failure.log)保留了后续失败。调试日志确认旧进程 goodbye 被缓存为 1 秒 TTL，新进程在其到期前发布相同记录；锁定的 mdns-sd 0.21.7 只刷新 TTL，未重新发出 ServiceResolved。广播实例名与主机名改为进程级随机后缀，持久设备 ID 不变。另一次[等待条件失败](evidence/mac-smoke-discovery-wait-failure.log)来自把已有加密连接的在线状态当作 mDNS 地址已齐；脚本已改为等待实际地址与端口，保留原字段断言。

独立审查发现消息转义后的帧上限、异常时间、配对超时清理、发现容量、Unicode 广播名、握手名额、配额回收和损坏发送快照问题。以上问题与后续发现的上传名额竞争、发送源丢失配额及旧配对确认竞争均已修复，最终独立复核未发现剩余 P1/P2。记录见[独立审查](evidence/review.md)。脚本审查还修正了停止进程时吞掉枚举失败，以及 Windows 启动日志与卸载说明。

## 最终候选实测

此前最终候选轮验证了快速重启后的 mDNS 发现、明确离线与重新上线、双向消息、双方重启后历史、离线重投、双向 1 GiB 文件、取消、拒收和撤销。完整文件哈希为 `7ff84aa8ddbc0d6c8e92d97bd90129a50d09deaf59f02626d5e9981ad76ee09b`。另一份 1 GiB 文件从日志证明的 3 MiB 持久偏移自动续传，完成哈希为 `05243dd09dc259e1f1c70ea0ff6027c852126edfe226169dfe18da2905ab90ca`。

测试还在已隔离的发送快照中修改一个字节。接收端校验失败，完整目标未发布，随后新聊天仍成功。未跳过校验或放宽断言。

macOS PKG 已展开并验证内含应用的 ad-hoc 签名。应用已复制安装到 `/Applications/LANLink.app`，真实启动后健康 API 返回成功，网页连接成功。可选登录启动经过 `launchctl bootstrap` 和健康检查；卸载辅助脚本成功关闭登录启动并停止 Agent，保留 SQLite 与文件目录。测试后恢复 Agent，登录启动保持关闭，应用保留供使用。没有执行管理员 PKG 安装或将应用移入废纸篓。[打包日志](evidence/mac-package.log)保留了工具输出的 `write: Permission denied` 警告；打包退出码为 0，随后对展开的应用执行 `codesign --verify --deep --strict` 通过。

| 产物              | SHA-256                                                            |
| ----------------- | ------------------------------------------------------------------ |
| Apple Silicon PKG | `c5f3e161a8ec6439b40a0de043b7bba705ffa79e67f9ea7a08ba7185656693e3` |
| Apple Silicon ZIP | `4957fb5324786c115f495019e5ece8eec0e63ddf0880f46a7d11f80f4e0508da` |

![安装后的网页入口](evidence/installed-mac.jpg)

## 发布门槛

仍需 Windows 原生构建、安装与卸载证据，真实双机发现、配对、文本、双向 1 GiB 文件和 IP 改变验证。macOS 开发包采用 ad-hoc 签名，未提供 Developer ID 签名或 Apple 公证；Windows 开发脚本未签名。完成核心验收前，所有产物只称为开发预览版本。
