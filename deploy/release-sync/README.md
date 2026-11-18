# 官网安装包自动发布

将包含四个安装包及 `SHA256SUMS` 的 GitHub Release 发布到 `JackEngineer/LanLink`。标签使用 `vX.Y.Z` 或 `vX.Y.Z-preview.N`，预览标签应勾选 GitHub 的预发行版本。先把文件上传到草稿，再发布 Release。发布后请保持该标签的文件不变，修订请发布新标签。

腾讯云每 15 分钟检查一次公开 Release。同步先验证 GitHub 的文件大小及可用的 SHA-256 摘要，再核对 `SHA256SUMS` 覆盖四个包并重新读取服务器上的文件进行校验。同一轮发现多个新品时，服务器先验证整批，再一次切换首页和下载目录。首页选择最高的版本，整批的版本直链均可用。缺包、校验失败或暂存出错会保留在线版本，定时器下次重试。低于官网当前版本的补发标签不自动导入。仓库内的 README 和安装指南仍可链接到历史版本。

新目录继承所有历史的 `/downloads/<tag>/` 路径。历史安装包优先使用硬链接，同一文件的多个版本树共用磁盘内容。系统拒绝硬链接时复制文件，例如初次迁移已有 root 所有的包。历史文件应保持只读约定，任何手动官网部署也应保留当前下载目录里的全部版本。切换后校验公网首页、校验清单及安装包 HTTP 大小。公网校验失败会记日志，下一次运行会复查首页当前版本。已开始分发的版本直链继续保留。

## 在现有 Ubuntu 部署一次

要求 Python 3.9+。现有 `/var/www/lanlink/current` 应是指向 `releases` 内版本目录的符号链接，站点文件和安装包应允许部署账号及 Nginx 读取。服务仅下载公开数据，无需 GitHub 令牌。

```sh
id lanlink-release >/dev/null 2>&1 || sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin lanlink-release
sudo chown lanlink-release:lanlink-release /var/www/lanlink /var/www/lanlink/releases
sudo install -d -o root -g root -m 0755 /usr/local/libexec
sudo install -o root -g root -m 0755 deploy/release-sync/sync.py /usr/local/libexec/lanlink-release-sync.py
sudo install -o root -g root -m 0644 deploy/release-sync/lanlink-release-sync.service /etc/systemd/system/lanlink-release-sync.service
sudo install -o root -g root -m 0644 deploy/release-sync/lanlink-release-sync.timer /etc/systemd/system/lanlink-release-sync.timer
sudo systemctl daemon-reload
sudo -u lanlink-release /usr/bin/python3 -B /usr/local/libexec/lanlink-release-sync.py --check
sudo systemctl start lanlink-release-sync.service
sudo systemctl enable --now lanlink-release-sync.timer
sudo systemctl list-timers lanlink-release-sync.timer
```

`--check` 在暂存目录校验新发行版并清理暂存文件，当前版本保持不变。若 GitHub 的最高版本与官网一致，校验现有文件及公网链接。通过 `sudo journalctl -u lanlink-release-sync.service -n 100 --no-pager` 查看运行情况。排查中可用 `sudo systemctl stop lanlink-release-sync.timer` 暂停定时同步，用 `sudo systemctl start lanlink-release-sync.service` 重试。若服务器异常关机使隐藏的 `.sync-*` 暂存目录残留，停止服务和定时器后确认目录不在 `current` 路径上，再清理残留暂存目录。

手动回滚前先停用定时器，并确认正在运行的同步任务已结束。把已公开的版本下载目录保留在回滚后的站点中。重新开启定时器后，服务会校验这些文件并恢复最新版本的下载入口。
