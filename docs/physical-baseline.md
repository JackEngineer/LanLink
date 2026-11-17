# Mac 与 Windows 物理双机验收：基线

2026-11-17，两台物理电脑在同一局域网运行各自安装的 LANLink。双方配对、收发消息、双向传输 10 MiB 与 1 GiB 文件均已实测。强制结束 Mac 接收进程后，1 GiB 文件从 4 MiB 处自动续传。**基线未通过自动发现稳定性验收。** 本记录不代表 v0.1.0 发布验收完成。

## 测试环境与包

| 电脑                             | 运行的 Agent                                                                                                                                                           | SHA-256                                                            |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| Apple Silicon Mac，macOS 27.0    | `LANLink.app` 中的 `lanlink-agent`。已记录的 Agent 源码为 `a15025c0407bd668fa3b1378393964f1984645b0`，与 Windows 基线的 `crates/`、`Cargo.toml` 和 `Cargo.lock` 一致。 | `383d4358389f08a6a4b55386c4b58db0510410528489f32d70db4cc3b59106ba` |
| Windows 10 22H2，19045.6466，x64 | `Install.ps1` 安装的 `lanlink-agent.exe`。源码 `4de3d6a1cc1bc01690819797a39d0612c1a089d4`，GitHub Actions run `44066092428`，artifact `14320896773`。                  | `3a64f7eff9214ffd8563ce581087dd649f52285e5761920e35ce8b3d3a9b3b49` |

Windows ZIP 的 SHA-256 为 `15939147da814ab543b8df66af1b8ac4ed7e736554e5ac3373ac4b039d19dc24`。Windows 防火墙保持开启，沿用已有程序规则。安装后本机健康接口返回 `ok`，网页显示本地服务已连接。见 [Windows 环境记录](evidence/physical-baseline/windows-environment.md)。

## 配对、聊天和重启

两端读到相同的配对安全码并分别确认。未配对和仅 Mac 确认时，发送请求均返回 HTTP 400，Windows 此时仍为 `paired=false`。双方确认后，每个方向的测试消息在两端各出现一次，消息 ID 一致。Mac 随后从网页发出一条中文消息，界面显示已送达。见 [Mac 配对画面](evidence/physical-baseline/mac-pairing.jpg)、[Mac 聊天画面](evidence/physical-baseline/mac-chat.jpg)、[Mac API 摘录](evidence/physical-baseline/mac-api-excerpt.jsonl)、[Windows 单边确认状态](evidence/physical-baseline/windows-pairing-one-confirmation.json)和 [Windows 拒绝响应](evidence/physical-baseline/windows-pairing-and-download-rejections.txt)。

Mac 正常停止后，Windows 显示对端离线。此时提交的消息 `65d742c0-658f-4d06-967f-618218699b68` 保持 `pending`。Windows 重启 Agent 后，设备身份、配对、消息历史、传输记录，以及这条待发消息均保留。Mac 恢复后，Windows 自动把同一条消息标记为 `sent`，Mac 只收到一次，无需手动重试。见 [双机重启状态](evidence/physical-baseline/windows-restart.json)和 [Mac 进程记录](evidence/physical-baseline/mac-lifecycle.jsonl)。

## 传输结果与断点

两端的 10 MiB 和 1 GiB 传输均为 `completed`。接收端对最终落盘文件另算 SHA-256，逐项与发送端一致。接收文件仍处于待接受状态时，下载请求返回 HTTP 400。

| 方向和大小             | 传输 ID                                | 接收端实测 SHA-256                                                 |
| ---------------------- | -------------------------------------- | ------------------------------------------------------------------ |
| Mac 到 Windows，10 MiB | `3dc339bf-c9ec-46f6-ae2f-3c1e4c5497a6` | `52224e9cdef8f53a8e80e160f6e3799d817d0790423f0041a72d124049a5be99` |
| Windows 到 Mac，10 MiB | `1102e7eb-80b5-4694-9064-a76ff1a2dd50` | `04875e27eb769abc55597ac75bf6a4bffd477304a29abb09d85278c560701be5` |
| Mac 到 Windows，1 GiB  | `384cdc33-4076-4b50-89c2-bb2b41125773` | `9abc035488a85bdd2237e51ed32481c8bba1a326e2730145d1ac1971d52ff19e` |
| Windows 到 Mac，1 GiB  | `fbbc053f-61e1-4dad-b977-74cd48f7e90b` | `9674bda70c01181b475879a0bd0ea2847d55335fb592fff65f423f4e36d5d859` |

证据：[Mac 源文件哈希](evidence/physical-baseline/mac-source-fixtures.json)、[Mac 接收文件哈希](evidence/physical-baseline/mac-received-disk-sha256.json)、[Windows 接收文件哈希](evidence/physical-baseline/windows-received-disk-sha256.json)、[Windows 完成状态](evidence/physical-baseline/windows-baseline-transfers.json)、[Mac 传输进度](evidence/physical-baseline/mac-transfer-progress.jpg)、[Windows 双向进度](evidence/physical-baseline/windows-transfer-progress.png)和 [Windows 完成画面](evidence/physical-baseline/windows-transfer-completed.png)。

续传使用另一条 Windows 到 Mac 的 1 GiB 任务 `e1020e93-261b-4072-8c61-7c4c43078a9d`。强制结束 Mac 接收进程时，API 偏移、SQLite 持久偏移和临时文件长度均为 4,194,304 字节，完整文件尚未发布。Windows 发送进程保持 PID `30612`，报告任务暂停。Mac 重启后，同一发送进程从 4,194,304 字节自动继续。任务完成后，Mac 最终文件为 1,073,741,824 字节，SHA-256 与上一行相同。见 [Mac 中断记录](evidence/physical-baseline/mac-lifecycle.jsonl)、[Windows 续传偏移日志](evidence/physical-baseline/windows-resume-offset.log)和 [续传状态采样](evidence/physical-baseline/windows-resume-samples.json)。

## Windows 原生安装生命周期

原包测试了登录启动项的创建和移除。执行 `Uninstall.ps1` 后，Agent 停止，本机 HTTP 不可达，安装目录和快捷方式移除，SQLite 与选定的 1 GiB 接收文件保留。重装同一 ZIP 后，EXE 哈希、设备身份、原配对状态、4 条聊天记录和 8 条传输记录均与卸载前一致。测试未注销或重启 Windows，尚未验证真实登录触发。见 [Windows 生命周期结果](evidence/physical-baseline/windows-install-lifecycle.json)。

## 基线未通过的项目

初次 mDNS 发现成功，两端随后都出现 `addresses=[]` 和 `port=0`，而已建立的加密连接仍然在线。重启后一度重新发现。基线应用每 20 秒调用一次 mDNS `verify`，给记录设置 10 秒的验证期限；在此期间未收到组播刷新时，端点可以在广播的 120 秒 TTL 到期前被移除。Windows 在失败窗口记录了从 WLAN 发出响应及本机组播回环，日志无法确认响应抵达 Mac 网卡。见 [Mac API 摘录](evidence/physical-baseline/mac-api-excerpt.jsonl)、[Windows 端点缺失状态](evidence/physical-baseline/windows-neighbor-after-transfers.json)和 [Windows 失败窗口日志](evidence/physical-baseline/windows-mdns-failed-window.txt)。

发现修复候选的物理回归、双机撤销授权（T14）和真实 IP 变化后的重新发现（T15）均不在这份基线记录中。

附件只保留本次验收所需的日志和截图。文本中的用户路径、私网地址与完整设备 ID 已换成占位符，截图保留测试设备短名及已过期的配对安全码。
