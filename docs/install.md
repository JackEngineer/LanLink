# 安装与使用 LANLink

## 取得开发预览包

从 [GitHub Release v0.1.0-preview.1](https://github.com/JackEngineer/LanLink/releases/tag/v0.1.0-preview.1) 选择与你的电脑匹配的安装包。

| 平台                        | 下载包                                                                                                                                                                                                                                               |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Apple Silicon Mac（M 系列） | [ZIP](https://github.com/JackEngineer/LanLink/releases/download/v0.1.0-preview.1/LANLink-0.1.0-aarch64-apple-darwin.zip) 或 [PKG](https://github.com/JackEngineer/LanLink/releases/download/v0.1.0-preview.1/LANLink-0.1.0-aarch64-apple-darwin.pkg) |
| Intel Mac                   | [ZIP](https://github.com/JackEngineer/LanLink/releases/download/v0.1.0-preview.1/LANLink-0.1.0-x86_64-apple-darwin.zip)                                                                                                                              |
| Windows x64                 | [ZIP](https://github.com/JackEngineer/LanLink/releases/download/v0.1.0-preview.1/LANLink-0.1.0-x86_64-pc-windows-msvc.zip)                                                                                                                           |

在 Mac 上可通过苹果菜单中的「关于本机」确认芯片或处理器型号。Apple Silicon 的 ZIP 和 PKG 两种格式任选其一。

安装包校验值见 [SHA256SUMS](https://github.com/JackEngineer/LanLink/releases/download/v0.1.0-preview.1/SHA256SUMS)。Intel Mac 包的验证范围为 CI 构建与测试。实机测试范围见[验收报告](review-fixes.md)。

## macOS

1. 按上表下载与本机匹配的开发预览包。
2. 安装 PKG，或将 ZIP 中的 `LANLink.app` 放到 `/Applications`。
3. 打开 LANLink。启动器会运行后台 Agent 并打开本机浏览器。
4. 将两台电脑连接到同一受信任局域网。
5. 在设备列表发起配对，在两台电脑核对完整安全码后分别确认。

开发版尚未公证。不要为了运行它全局关闭 Gatekeeper。需要可公开无提示安装的版本时，发行者需完成 Developer ID 签名和公证。

如需登录时启动，执行 `bash /Applications/LANLink.app/Contents/Resources/login-start.sh enable`。将 `enable` 改为 `disable` 关闭登录启动。

执行 `bash /Applications/LANLink.app/Contents/Resources/uninstall.sh` 关闭登录启动，脚本会停止该安装路径的 Agent。随后手动将应用移到废纸篓。卸载脚本保留本地历史和文件。默认数据目录为 `~/Library/Application Support/dev.LANLink.LANLink`，Agent 日志在其 `logs` 子目录，启动器日志在 `~/Library/Logs/LANLink/launcher.log`。

## Windows

1. 解压 x64 ZIP 到临时目录。
2. 在 PowerShell 中运行 `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\Install.ps1`。
3. 如需登录启动，在同一命令末尾添加 `-StartAtLogin`。
4. 从开始菜单打开 LANLink。
5. 防火墙提示出现时，只允许受信任的专用网络。

安装不自动添加允许公网访问的防火墙规则。设备发现使用 UDP 5353，设备传输使用 TCP 38762 或 Agent 实际广播的端口。路由器的客户端隔离和访客 Wi-Fi 可能阻止设备互访。

在安装目录之外打开 PowerShell，运行 `powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\LANLink\Uninstall.ps1"` 可停止当前安装的 Agent 并删除程序与快捷方式。身份、历史和接收文件会保留。

Windows 默认数据目录为 `%LOCALAPPDATA%\LANLink\LANLink\data`。Agent 日志和启动器的 `launcher.stdout.log`、`launcher.stderr.log` 位于其 `logs` 子目录。

## 消息与文件

配对后选择设备发送消息。`sent` 表示接收端 Agent 已将消息保存到数据库，不表示对方已阅读。断线后未确认的消息保留原 ID 重试。

发送文件后，接收端需要单独接受。默认单文件上限为 2 GiB。传输完成前不会以完整文件对外发布。断线后保留已确认偏移，重新连接时继续传输。取消或拒绝后不自动恢复。

## 排查连接

确认两台设备在同一个局域网，Agent 都在运行，防火墙允许专用网络访问，且路由器未启用客户端隔离。设备异常断电或被强制结束时，发现记录可能保留到 mDNS 的 120 秒 TTL 到期；正常退出会主动发送离线通知。浏览器入口始终是本机 `127.0.0.1`，不要输入另一台设备的 IP 来访问它的 Web UI。

Windows 开发版脚本未签名。上述执行策略只作用于当前安装进程，不修改系统策略；组织 Group Policy 的限制仍然生效。正式发行的安装器签名仍待完成。

Windows 登录启动可通过重新运行安装命令并追加 `-DisableStartAtLogin` 关闭。升级安装会先停止当前安装路径的 Agent；进行中的传输需等待重新连接后恢复。

## 构建平台

构建流程配置 Apple Silicon、Intel Mac 和 Windows x64 三个任务。macOS 标签按 [GitHub 官方运行器表](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)选择 `macos-15` 与 `macos-15-intel`。任务配置存在不代表已通过，实际结果以验收报告为准。
