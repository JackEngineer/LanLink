# Mac 与 Windows 物理双机验收

2026-11-17，在同一局域网上使用 Apple Silicon Mac（macOS 27.0）和 Windows 10 22H2 x64（19045.6466）。发现修复候选完成了双向发现、双向消息和双向 1 GiB 文件回归，同时验证了 Windows 强制停止后的自动恢复，以及 Mac 撤销授权、重启和重新配对。[首次双机验收及 mDNS 故障记录](physical-baseline.md)

## 实机候选

| 电脑              | 来源                                                            | 已安装 Agent SHA-256                                               |
| ----------------- | --------------------------------------------------------------- | ------------------------------------------------------------------ |
| Apple Silicon Mac | `4e8db4eb445435388b2fd6494bfa9b32796ff479`，本机打包            | `bea9e5ce87c6f251f4d5717120cbb05549a658264122e9b4c70f69c975e28547` |
| Windows x64       | `fad384cf32187928f260d99efec1bb10de19c873`，GitHub Actions 构建 | `057db3721886e7d08562eed9560e14d92a431d9051c66bf43160693329570d1e` |

两个提交之间只修改了 `scripts/smoke.py`。GitHub Actions 的 Windows x64、Apple Silicon Mac 和 Intel Mac 作业均成功。[候选身份及安装包校验](evidence/physical-final/candidate-binding.md) · [Windows 安装记录](evidence/physical-final/windows-installed.json) · [CI run 44074931608](https://github.com/JackEngineer/LanLink/actions/runs/44074931608)

Windows 最终证据通过 LANLink 传输，任务 ID 为 `baf1c4a8-de0a-4260-9a0f-3f0f5a95a8ed`，接收端落盘 SHA-256 为 `962cb15413b4be6d36d014dc10d5376ad5e6761e94eebe3a4a72723b5647a511`。归档前核对了包内清单的 22 项文件长度和 SHA-256。公开文本保留采样和状态，私网地址和本机路径已脱敏，设备 ID 保留前八位。

## 发现稳定性（T03）

旧版每 20 秒调用一次 mDNS `verify`，将验证期限设为 10 秒。漏收组播响应时可能提前删除广播 TTL 为 120 秒的端点。修复移除该调用，保留 mDNS 缓存的正常刷新、TTL 过期和 goodbye 处理。隔离双 Agent 复跑中，基线在静默 29.1 秒后丢失端点，修复版在静默 35 秒后仍保留端点，正常停止后移除端点，重启后重新发现同一设备。开启网络访问后，27 项 Rust 测试通过。[发现回归日志](evidence/physical-final/discovery-regression.log)

| 观察端  | 对端    | UTC 采样窗口                                       | 样本 | 无效端点 |
| ------- | ------- | -------------------------------------------------- | ---- | -------- |
| Mac     | Windows | 11:32:35.412–11:36:40.141，首末样本相隔 244.729 秒 | 123  | 0        |
| Windows | Mac     | 11:41:39.165–11:45:40.670，首末样本相隔 241.506 秒 | 121  | 0        |

每次采样均检查对端地址与服务端口。两个采样窗口均超过两倍 120 秒 TTL。[观察汇总](evidence/physical-final/discovery-soak-summary.json) · [Mac 采样](evidence/physical-final/mac-discovery-samples.jsonl) · [Windows 采样](evidence/physical-final/windows-discovery-240s.jsonl) · [Windows 采样汇总](evidence/physical-final/windows-discovery-summary.json)

## 消息与文件（T07、T10、T12）

两端 API 均记录了双方各发一条测试消息。Windows 到 Mac 的消息 ID 为 `b37ee4f3-571f-4de7-b45a-4453be213272`，Mac 到 Windows 为 `3e3f182e-2dfa-415e-bd38-c79c84340670`，状态均为 `sent`。[Mac 消息与传输记录](evidence/physical-final/candidate-messages-and-files.json) · [Windows 消息](evidence/physical-final/windows-messages.json)

两条文件任务在两端均为 `completed`，各接收 1,073,741,824 字节。接收端分别重算落盘文件的 SHA-256，均与发送端一致。[Windows 完成记录](evidence/physical-final/windows-transfers-latest.json) · [Mac 落盘校验](evidence/physical-final/mac-final-file-hash.json) · [Windows 落盘校验](evidence/physical-final/windows-received-1GiB-disk-verified.json)

| 方向           | 传输 ID                                | 接收端落盘 SHA-256                                                 |
| -------------- | -------------------------------------- | ------------------------------------------------------------------ |
| Windows 到 Mac | `5bcdc711-6c71-4a35-bd8d-e5831e259205` | `9674bda70c01181b475879a0bd0ea2847d55335fb592fff65f423f4e36d5d859` |
| Mac 到 Windows | `6e2bc2ed-86bb-418c-86af-577d2ce792b4` | `9abc035488a85bdd2237e51ed32481c8bba1a326e2730145d1ac1971d52ff19e` |

未配对及单端确认时拒绝投递、双向 10 MiB、卸载重装后保留数据及从 4 MiB 自动续传由基线实机验收覆盖。[基线记录](physical-baseline.md)

## Windows 强制停止与重启（T04、T05、T08、T13）

11:50:16.357 UTC 强制结束 Windows Agent。Mac 于 11:51:53.639 首次采样到对端离线，比进程退出晚 97.282 秒，低于本轮 135 秒验收阈值。发现地址和端口已清除，配对仍在。[Windows 进程退出](evidence/physical-final/windows-process-hard-stop.json) · [Mac 离线采样](evidence/physical-final/windows-hard-stop.json)

Mac 在 11:52:06.528 提交消息 `e4fb5702-28fc-4f73-8319-860f46f1b3b1`，状态为 `pending`。Windows 于 11:52:35.379 启动新进程，同一消息随后自动送达，历史记录出现一次，状态为 `sent`。6 条既有消息和 13 条传输记录均保留，已安装 EXE 哈希一致。[Windows 重启身份](evidence/physical-final/windows-hardstop-recovery-identity.json) · [Windows 历史及送达检查](evidence/physical-final/windows-hardstop-preservation-check.json)

## 跨机撤销授权（T14）

Mac 于 11:53:34.612 UTC 撤销配对。本机再发消息和小文件均返回 HTTP 400。大文件探测因服务端提前关闭上传触发客户端 `BrokenPipe`，HTTP 状态未记录。Windows 仍持旧信任，在 10.102 秒观察期间消息保持 `pending`，文件保持 `awaiting_accept` 且偏移为 0。Mac 用同一安装二进制重启后，本地消息发送仍返回 HTTP 400。重启前后各 8 次采样均显示未配对，且无这两条探测记录。Windows 重查状态一致，随后取消探测文件。[Mac 撤销及重启采样](evidence/physical-final/revocation.json) · [Windows 探测采样](evidence/physical-final/windows-revoke-probe-observation.jsonl) · [Windows 重启后复查](evidence/physical-final/windows-revoke-after-mac-restart.json) · [文件取消](evidence/physical-final/windows-revoke-probe-cancelled.json)

Mac 重新发起配对。双方独立核对同一安全码并依次确认。重新授权后，Windows 待发探测消息才送达，双方设备列表恢复为已配对、在线。19:57（当地时间），Mac 收到 Windows 中文消息 `80710141-b17b-4ccb-927e-b0b65573f41c` 后从网页回复，回复在本机显示已送达并出现在 Windows 聊天记录中。[Mac 配对 API](evidence/physical-final/mac-re-authorized.jsonl) · [Windows 确认前状态](evidence/physical-final/windows-re-pair-before-confirm.json) · [Windows 重新授权](evidence/physical-final/windows-re-authorized.json)

两端 Agent 在验收结束时运行，登录启动均关闭。[Mac 启动日志](evidence/physical-final/mac-final-agent.log) · [Mac 登录项检查](evidence/physical-final/mac-login-disabled.json) · [Windows 安装记录](evidence/physical-final/windows-installed.json)

[Mac 聊天](evidence/physical-final/mac-final-chat.jpg) · [Windows 聊天](evidence/physical-final/windows-final-chat.png) · [Mac 文件传输](evidence/physical-final/mac-final-transfers.jpg) · [Windows 文件传输](evidence/physical-final/windows-final-transfers.png)

## 受控 IP 迁移（T15）

在同一台 Mac 上，用已安装候选运行两个隔离 Agent，通过真实 mDNS、TCP、Noise 和 SQLite 验证端点迁移。保留 Agent 进程和设备身份，关闭私网 IPv4 地址上的测试 TCP 入口和 mDNS 广播，再用同一实例名、设备 ID 和端口公布回环地址。旧端点继续拒绝 TCP，发送 Agent 自动发现新地址、重连并送达待发消息，接收端 SQLite 只保留一条该消息。受控迁移及独立复核通过，测试进程和测试广播均已退出。物理双机 DHCP 换址和网卡切换未测。[受控迁移日志](evidence/physical-final/ip-migration.log) · [本机集成回归](evidence/physical-final/migration-helper-smoke.log)

以上记录对应本次发现修复候选。随后评审发现的文件上传及传输错误处理问题，需在新候选上重新验证。

当前产物为开发预览。Apple Silicon 包使用 ad-hoc 签名，Developer ID 签名与 Apple 公证未完成，Windows 开发脚本未签名。Intel Mac 覆盖 CI 构建与测试，尚无实机验收。真实注销重登录尚未验证，内存、启动、发现时延和 ACK 性能目标尚未测量。
