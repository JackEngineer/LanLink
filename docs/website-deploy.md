# 在已有腾讯云服务器上部署 LANLink 官网

官网是独立静态站点，构建产物为 `dist/site`，部署包为 `dist/lanlink-website.tar.gz`。它不运行 Agent，不连接本机 API，也不使用 `apps/web` 的应用构建产物。安装包和校验文件通过官网的版本化路径 `/downloads/<版本>/` 提供。

安装包和下载入口的版本更新由[官网自动发布服务](../deploy/release-sync/README.md)处理。下文适用于首次部署和手动更新网站页面。

本文使用已有 Nginx 的 Linux 服务器和独立域名。示例中的 `NGINX_BIN` 与 `NGINX_CONF` 必须先按实际进程修改；如果 master 使用 `-p`，后续每条命令也要带相同前缀参数。所有 `example.com`、SSH 主机和发布目录标识都必须替换。实际发布前先确认服务器、域名、现有站点清单和发布授权。不要覆盖已有站点配置、默认站点或其他应用目录。

## 1. 构建并检查部署包

在仓库根目录使用 Node.js 24 和 npm 执行。官网本身不需要安装额外依赖。

```sh
npm run site:check
npm run site:package
tar -tzf dist/lanlink-website.tar.gz
shasum -a 256 dist/lanlink-website.tar.gz
```

部署包根目录包含 `index.html`、`styles.css` 和 `assets/`。检查命令会验证实际构建产物的本地资源、锚点和下载链接，并与 `apps/site/release.json`、README、安装指南比较。升级预览版本时，一起更新这些来源，再重新构建。

## 2. 上传到独立发布目录

以下示例发布标识是 `20261118-01`。每次部署使用新的标识。先检查目标目录未存在，避免覆盖旧版本。上传目的地为已授权 SSH 用户的临时目录。

```sh
scp dist/lanlink-website.tar.gz deploy@example.com:/tmp/lanlink-website-20261118-01.tar.gz
```

登录服务器，先检查正在提供公网服务的 Nginx master 进程、启动参数、二进制位置、配置路径及配置中的 `pid`。同一台机器可能同时运行多个实例。必须使用公网实例相同的二进制、`-c`、`-p` 等参数执行检查和重载，不能假定系统服务就是目标实例。保存当前配置及 `current` 指向，记录部署包哈希，并与本地结果比较。

```sh
ps -eo pid,args
NGINX_BIN=/usr/sbin/nginx
NGINX_CONF=/etc/nginx/nginx.conf
sudo "$NGINX_BIN" -c "$NGINX_CONF" -T
sha256sum /tmp/lanlink-website-20261118-01.tar.gz
sudo test ! -e /var/www/lanlink/releases/20261118-01
sudo install -d -m 755 /var/www/lanlink/releases/20261118-01
sudo tar -xzf /tmp/lanlink-website-20261118-01.tar.gz -C /var/www/lanlink/releases/20261118-01
sudo find /var/www/lanlink/releases/20261118-01 -type d -exec chmod 755 {} \;
sudo find /var/www/lanlink/releases/20261118-01 -type f -exec chmod 644 {} \;
readlink -f /var/www/lanlink/current
```

首次部署时 `current` 尚不存在，最后一条命令可能没有输出。确认 Nginx 工作进程可以读取新目录。不要递归更改 `/var/www` 或其他站点的权限。

在切换 `current` 前，将现有站点的 `downloads/` 目录复制到新发布目录，保留已发布版本的直链。首次迁移或发布新版本时，将经过校验的安装包与 `SHA256SUMS` 放入新发布目录的 `downloads/<版本>/`。目录和文件分别使用 `0755` 和 `0644` 权限。

在新版本目录执行 `sha256sum -c SHA256SUMS`，再核对文件名与 `apps/site/release.json`。切换后通过官网 HTTPS 下载每个安装包并重新校验。若需要回滚，先把回滚目录缺少的已公开版本复制过去，确保已分享的下载地址继续可用。

## 3. 配置独立域名和 TLS

将域名 DNS 指向这台服务器。在腾讯云安全组及服务器防火墙中按现有策略开放 HTTP 和 HTTPS。中国内地的服务器使用域名前，先确认域名备案及接入要求。

以 `deploy/nginx/lanlink.conf` 为模板，替换 `example.com`。通过现有 Nginx 的配置机制加载独立文件，例如 `/etc/nginx/conf.d/lanlink.conf`。若服务器已有同名文件，先备份并审阅差异。不要假设此目录适用于所有安装方式。Nginx 的配置与重载方式见[官方入门文档](https://nginx.org/en/docs/beginners_guide.html)。

模板只包含 HTTP，方便先检查域名路由。正式上线前，使用该服务器现有的证书工具申请并安装对应域名证书，建立单独的 `listen 443 ssl` 站点，保留模板中的 `root`、静态文件规则和安全头。把 80 端口站点改成跳转到固定的正式 HTTPS 域名；若证书续期使用 HTTP 验证，保留相应验证路径。确认续期机制有效。LANLink 当前使用的 Certbot 自动续签及部署方式见[自动续签说明](../deploy/certbot/README.md)。仅在 HTTPS 正常并确定域名范围后配置 HSTS。

如果 Nginx 由带文件系统沙箱的服务管理器运行，先确认服务允许写入的日志目录。为 LANLink 创建独立日志文件时，沿用允许写入的目录，不放宽服务沙箱。主机终端中的 `nginx -t` 不包含服务的挂载限制；Linux 上可在已核实 master 的挂载命名空间内使用 `nsenter` 复验配置。

本站没有客户端路由。未知路径必须返回 404，不要设置 `try_files ... /index.html` 这类 SPA 兜底。HTML 不缓存，CSS 与 SVG 缓存五分钟，更新后不会被一年期 `immutable` 缓存锁住。若在站点层修改响应头，注意 Nginx `add_header` 的继承规则，继续保留模板里的安全头。

## 4. 切换、检查配置并重载

先记录旧 `current` 的实际目标。`current` 应是 LANLink 专用符号链接，不能是其他站点目录。下面以同目录临时链接再原子替换，要求目标服务器提供 GNU `mv -T`。

```sh
sudo ln -s /var/www/lanlink/releases/20261118-01 /var/www/lanlink/current.next
sudo mv -Tf /var/www/lanlink/current.next /var/www/lanlink/current
sudo "$NGINX_BIN" -c "$NGINX_CONF" -t
sudo "$NGINX_BIN" -c "$NGINX_CONF" -s reload
```

每条命令成功后再执行下一条。若配置检查失败，停止发布，恢复本次配置修改与原 `current` 指向，再重新检查；不要 reload。重载前核对目标配置的 pid 文件确实指向已确认的 master。不要直接执行 `systemctl reload nginx`，它可能操作另一个实例。只有已经确认服务单元与目标实例一致时，才能使用该服务管理方式。

reload 命令成功只表示已发送信号。继续检查目标 master 的新 worker 启动时间、错误日志和真实 HTTPS 响应，再判定新配置已生效。[官网上线验收记录](website-acceptance.md)保留了本次沙箱日志目录问题及修正结果。

```sh
curl -I https://example.com/
curl -I https://example.com/styles.css
curl -I https://example.com/assets/lanlink.png
curl -I https://example.com/assets/demo-request.webp
curl -I https://example.com/this-path-must-not-exist
```

首页和静态资源应返回 200，未知路径应返回 404。确认 HTTPS 证书、内容类型、缓存和安全头。浏览器检查桌面与手机布局、锚点、FAQ、三个平台的四个下载链接，以及安装指南和校验值链接。验证其他既有站点仍可访问。

## 5. 回滚

保留旧发布目录和本次修改前的 Nginx 配置。发现问题时，将 `current` 原子切回之前记录的发布目录。以下旧标识只是示例，必须换成服务器上实际保留的版本。

```sh
sudo ln -s /var/www/lanlink/releases/20261117-01 /var/www/lanlink/current.rollback
sudo mv -Tf /var/www/lanlink/current.rollback /var/www/lanlink/current
sudo "$NGINX_BIN" -c "$NGINX_CONF" -t
sudo "$NGINX_BIN" -c "$NGINX_CONF" -s reload
```

如本次改过 Nginx 配置，先恢复其备份再检查与 reload。首次上线没有旧发布版本时，禁用本次新增的独立站点配置，检查后 reload。再次验收官网及其他站点。不要自动删除历史发布目录，清理前确认它不是当前或回滚目标。

## 可选的静态托管备份

如以后需要 COS 备用入口，先配置自定义域名与 HTTPS，不要直接以默认桶域名作为官网入口。具体限制见[腾讯云 COS 静态网站配置说明](https://cloud.tencent.com/document/product/436/102509)。当前交付只准备已有服务器的 Nginx 部署，不创建 COS 资源。
