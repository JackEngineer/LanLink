# Windows 基线环境

摘自 Windows 端 2026-11-17 验收记录。

- Windows 10 Home China 22H2，build 19045.6466，x64。
- 源码提交 4de3d6a1cc1bc01690819797a39d0612c1a089d4，GitHub Actions run 44066092428，artifact 14320896773。
- ZIP SHA256 15939147da814ab543b8df66af1b8ac4ed7e736554e5ac3373ac4b039d19dc24。
- 当前运行 EXE SHA256 3a64f7eff9214ffd8563ce581087dd649f52285e5761920e35ce8b3d3a9b3b49。安装目录 %LOCALAPPDATA%\Programs\LANLink，数据目录 %LOCALAPPDATA%\LANLink\LANLink\data。
- 初始PID15112，受控重启后PID30612。断点恢复全程PID30612不变。
- 实际运行Install.ps1成功，health ok，网页显示本地服务已连接。HTTP仅127.0.0.1:38761，LAN TCP38762。未启用登录启动。
- 防火墙保持开启。只读查到程序级Private允许TCP和UDP、Public阻止TCP和UDP规则。本验收未修改规则。
