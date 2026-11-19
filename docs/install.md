# 安装与使用 LANLink

## 下载正式版

从 [LANLink 官网 v0.2.0](https://lanlink.heyyami.com/#download) 选择与你的电脑匹配的安装包。

| 平台                        | 下载包                                                                                                                                                                                   |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Apple Silicon Mac（M 系列） | [ZIP](https://lanlink.heyyami.com/downloads/v0.2.0/LANLink-0.2.0-aarch64-apple-darwin.zip) 或 [PKG](https://lanlink.heyyami.com/downloads/v0.2.0/LANLink-0.2.0-aarch64-apple-darwin.pkg) |
| Intel Mac                   | [ZIP](https://lanlink.heyyami.com/downloads/v0.2.0/LANLink-0.2.0-x86_64-apple-darwin.zip)                                                                                                |
| Windows x64                 | [ZIP](https://lanlink.heyyami.com/downloads/v0.2.0/LANLink-0.2.0-x86_64-pc-windows-msvc.zip)                                                                                             |

在 Mac 上可通过苹果菜单中的「关于本机」确认芯片或处理器型号。Apple Silicon 的 ZIP 和 PKG 两种格式任选其一。

安装包校验值见 [SHA256SUMS](https://lanlink.heyyami.com/downloads/v0.2.0/SHA256SUMS)。Intel Mac 版已通过 CI 构建与测试。实机结果见[一键升级验收](acceptance.md#v020-一键升级实机验收)。

## 让 AI 助手协助安装

官网[下载区](https://lanlink.heyyami.com/#download)默认选中「通过 AI Agent 安装」。点击「复制」，即可复制带有官网安装步骤链接的指令。将提示粘贴给当前电脑上能操作文件和终端的 AI 助手，由它判断系统与架构、检查已有安装、下载正式版并校验 SHA-256。浏览器不会执行安装，也不会向 AI 服务发送信息。

复制失败或关闭 JavaScript 时，可以选中指令手动复制。展开「查看完整安装步骤」可查看助手将按哪些步骤操作。助手遇到安全拦截、权限确认或安装失败应停止，将决定交还给你。安装后的设备配对和安全码仍需本人确认。

## macOS

1. 按上表下载与本机匹配的安装包。
2. 安装 PKG，或将 ZIP 中的 `LANLink.app` 放到 `/Applications`。
3. 打开 LANLink。启动器会运行后台 Agent 并打开本机浏览器。
4. 将两台电脑连接到同一受信任局域网。
5. 在设备列表发起配对，在两台电脑核对完整安全码后分别确认。

macOS 安装包采用 ad-hoc 签名，尚未提供 Developer ID 签名和 Apple 公证。请保留 Gatekeeper 的保护。

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

## 升级 LANLink

从 `v0.2.0-preview.4` 或 `v0.2.0-preview.5` 升级时，在桌面 LANLink 打开「设置」，在「软件更新」中点击一次「一键升级」。LANLink 从官网下载安装包，按 GitHub Release 的 SHA-256 校验后安装并重启。当前浏览器标签页会自动刷新。

升级需要连通 LANLink 官网和 GitHub。下载和校验期间可以继续使用 LANLink。重启会短暂断开连接，桌面文件传输会按已保存的进度恢复，手机扫码会话需要重新连接。升级沿用原来的设备身份、消息、文件和登录启动设置。

安装目录需要允许当前账户写入。如果旧版没有一键升级功能，请从上表下载正式版并覆盖安装。

## 消息与文件

配对后选择设备发送消息。`sent` 表示接收端 Agent 已将消息保存到数据库，不表示对方已阅读。断线后未确认的消息保留原 ID 重试。

发送文件后，接收端需要单独接受。默认单文件上限为 2 GiB。传输完成前不会以完整文件对外发布。断线后保留已确认偏移，重新连接时继续传输。取消或拒绝后不自动恢复。

## 与手机或平板扫码传文件

1. 将电脑和手机或平板连到同一个可信 Wi‑Fi。在电脑的 LANLink 页面打开「手机互传」，再点击「生成二维码」。如果有多个局域网地址，选择连接到这个 Wi‑Fi 的地址。
2. 用手机或平板扫描二维码，在浏览器中打开传输页。在电脑上输入手机页面的 6 位确认码，再点击「允许连接」。默认勾选「记住此浏览器」，90 天内用同一个浏览器再次扫码可直接连接。临时借用的设备可以取消勾选。
3. 任选一端选择文件。手机上传的文件保存在电脑的接收目录，电脑选好的文件可以在手机浏览器里下载。
4. 传完后在电脑上结束会话。会话到期也会关闭。传输期间电脑需要保持在线，已经保存到接收目录的文件会保留。

扫码传输使用局域网 HTTP，传输未加密。每次会话只允许一台手机或平板连接，有效期为 10 分钟，最多收发 16 个文件，合计最多 2 GiB。单文件还受电脑上的接收大小设置限制。中断的传输需要重新选择文件。

「手机互传」中会列出已记住的浏览器。点击「忘记」会撤销其授权，并断开该浏览器的当前互传会话。下次扫码需要重新确认。更换浏览器、使用无痕窗口、清除网站数据，或电脑的局域网 IP 改变后，也可能需要重新确认。

## 排查连接

确认设备在同一个局域网，电脑上的 Agent 正在运行，防火墙允许专用网络访问，且路由器未启用客户端隔离。电脑管理页面始终使用本机 `127.0.0.1`。扫码连接使用会话临时生成的地址和端口。设备异常断电或被强制结束时，发现记录可能保留到 mDNS 的 120 秒 TTL 到期；正常退出会主动发送离线通知。

Windows 安装脚本尚未签名。上述执行策略只作用于当前 PowerShell 进程。组织的组策略限制仍然生效。

Windows 登录启动可通过重新运行安装命令并追加 `-DisableStartAtLogin` 关闭。升级安装会先停止当前安装路径的 Agent；进行中的传输需等待重新连接后恢复。

## 构建平台

构建流程配置 Apple Silicon、Intel Mac 和 Windows x64 三个任务。macOS 标签按 [GitHub 官方运行器表](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)选择 `macos-15` 与 `macos-15-intel`。任务配置存在不代表已通过，实际结果以验收报告为准。
