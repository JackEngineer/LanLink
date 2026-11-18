# LANLink 官网上线验收

2026-11-18 北京时间，官网已发布到 [lanlink.heyyami.com](https://lanlink.heyyami.com/)。静态内容来自 `2985d71`，运行于已有腾讯云 Ubuntu 服务器的独立 Nginx 站点。

## 页面与发布包

- 独立审查、`npm run site:package`、Prettier 和 `git diff --check` 通过。
- 浏览器检查 375、768、1440 像素布局，修复平板装饰环横向溢出。下载锚点、原生 FAQ 和键盘跳转正文通过。
- 下载清单与 README、安装指南中的三个平台、四个安装包及校验值链接一致。GitHub Release 已确认公开，且标记为开发预览。
- 发布包仅含 HTML、CSS、SVG。关闭 Mac 资源叉与扩展属性打包后，Python tarfile 和 Ubuntu GNU tar 均未发现额外元数据文件。
- 页脚主体和 ICP 备案号取自当时可访问的 [heyyami.com 主站](https://heyyami.com/)，未宣称另行核验工信部实时备案状态。

最终部署包 SHA-256 为 `843720b02ceabb1d10fb8ee078e101df18c82f8b7201afa2b9050ce140872282`。公网实际返回的三个文件与本地发布内容逐字节一致，哈希如下。

| 文件               | SHA-256                                                          |
| ------------------ | ---------------------------------------------------------------- |
| index.html         | a25b989e7d7fe84f3bd7e5c6c0180227066ff3e91c2d5f043791ec50b065c0b7 |
| styles.css         | 88c4110049965fd7ab6d47c1823e219bcf222e4d63c7ca084ea91e312f7b810f |
| assets/lanlink.svg | 7129ad1d54789186ee57fe1a6351c9c0440c4a1a6dee9f22061b8f8afdb5f804 |

## 生产配置与验收

用户明确批准免费证书申请、自动 DNS 验证、子域名 A 记录及官网上线后执行。证书域名为 `lanlink.heyyami.com`，证书链和私钥匹配，私钥安装后为 root 所有、权限 600。首次上线的腾讯云证书有效期至北京时间 2027-02-15 22:59:59，当时未开启自动续签。后续自动续签配置及验收见下文。

公网 Nginx 使用专用配置和 pid，系统默认 Nginx 是另一实例。保留原配置备份，仅增加独立站点 include，以原 master 的配置执行检查和重载。

首次主机终端配置检查成功，但服务重载因日志目录不在服务沙箱的可写范围内而失败，HTTPS 仍由默认站点拒绝握手。修正为在现有允许写入的目录保存 LANLink 独立日志，在目标 master 的挂载命名空间中复验通过后再次重载。未放宽服务沙箱。已确认新 worker 启动，回环与公网 HTTPS 均返回正确官网。

实际验收结果如下。

- HTTPS 首页、CSS 和 SVG 返回 200，TLS 验证未关闭，浏览器正常打开。
- HTTP 返回 301 到固定 HTTPS 域名，未知路径返回 404。
- HTML 使用 `no-cache`，静态资源缓存五分钟。响应保留 CSP、nosniff、Referrer-Policy、X-Frame-Options 和 Permissions-Policy。
- 主站、青舟官网、管理入口、API 根路径和邮件入口的 HTTP 状态与发布前一致。此检查不代表业务流程或客户数据操作验收。

可重新执行以下只读检查。

```sh
curl -I https://lanlink.heyyami.com/
curl -I http://lanlink.heyyami.com/
curl -I https://lanlink.heyyami.com/styles.css
curl -I https://lanlink.heyyami.com/this-path-must-not-exist
```

官网仅提供介绍与 GitHub Release 下载入口。未修改 LANLink 本机应用、Agent 或安装包。

## 自动续签与自动部署

2026-11-18 按用户要求，官网已切换到现有 Certbot 管理的 Let's Encrypt 证书。使用已有 ACME 账户和 `/var/www/certbot`，为 LANLink 配置独立 HTTP 验证路径、renewal 配置和 deploy hook，未修改邮件证书的 renewal 配置。

`certbot.timer` 已启用。LANLink 配置持久化 `renew_hook = /usr/local/sbin/lanlink-cert-deploy`。钩子只接受精确的 LANLink lineage 和域名，检查证书与私钥后原子替换证书目录，在正确 Nginx master 的命名空间内检查并重载，通过可信 TLS 回环连接核对实际证书指纹。失败恢复旧证书。脚本及部署命令见[自动续签说明](../deploy/certbot/README.md)。

实际验证结果如下。

- 首次真实签发成功，deploy hook 输出证书已部署并验证。
- 公网证书颁发者为 Let's Encrypt YR1，当前到期时间为北京时间 2027-02-16 10:07:33。SHA-256 指纹为 `9b99d499b1ca49de0ac3447a69080e07ed1b86fbd9e2922e25ffa4f9a2012003`。
- `certbot renew --cert-name lanlink.heyyami.com --dry-run --run-deploy-hooks --no-random-sleep-on-renew` 返回 0，模拟续签及部署钩子均成功。演练未向官网安装测试环境证书。
- 原腾讯云证书保留在 root 专用备份目录，私钥未进入仓库。

定时器启用和一次演练通过不能证明未来所有续签都会成功。后续仍需保留服务器时间、DNS、80 端口挑战路径、CA 网络访问和日志检查能力。
