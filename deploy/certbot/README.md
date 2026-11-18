# 配置 LANLink 证书自动续签

在 Ubuntu 服务器执行以下步骤。使用已有 Certbot 账户及 `certbot.timer`，只为 `lanlink.heyyami.com` 创建独立证书。不要使用 Nginx 插件或修改其他域名的 renewal 配置。

1. 在 LANLink 的 HTTP 站点中保留 ACME webroot 路由，再对其他路径跳转 HTTPS。把 server 级别的固定 `return 301` 移进 `location /`，否则它会提前拦截验证请求。

```nginx
location ^~ /.well-known/acme-challenge/ {
	root /var/www/certbot;
	try_files $uri =404;
}
location / {
	return 301 https://lanlink.heyyami.com$request_uri;
}
```

2. 核对 `/run/qingzhou-public-website/nginx.pid` 对应 `/usr/sbin/nginx -c /etc/qingzhou-public-website/nginx.conf`。在该 master 的挂载命名空间内运行 `nginx -t`，然后只向该 master 发送 HUP。检查公网验证路径能读取 `/var/www/certbot` 中的测试文件。

3. 将本目录的脚本安装为 root 所有的可执行 deploy hook。脚本只处理精确匹配的 LANLink lineage 和域名，其他证书直接退出。

```sh
sudo install -o root -g root -m 0755 deploy/certbot/lanlink-deploy.py /usr/local/sbin/lanlink-cert-deploy
sudo certbot certonly --webroot -w /var/www/certbot --cert-name lanlink.heyyami.com -d lanlink.heyyami.com --non-interactive --deploy-hook /usr/local/sbin/lanlink-cert-deploy
```

4. 如果首次申请没有执行 deploy hook，手动安装已有生产证书。Nginx 继续读取 `/etc/lanlink-website/tls/fullchain.pem` 和 `privkey.pem`。

```sh
sudo env RENEWED_LINEAGE=/etc/letsencrypt/live/lanlink.heyyami.com RENEWED_DOMAINS=lanlink.heyyami.com /usr/local/sbin/lanlink-cert-deploy
```

5. 验证续签及 deploy hook，再检查公网证书、官网和其他已有站点。

```sh
sudo certbot renew --cert-name lanlink.heyyami.com --dry-run --run-deploy-hooks
sudo systemctl is-enabled certbot.timer
sudo systemctl list-timers certbot.timer
```

`--run-deploy-hooks` 在 dry run 成功后向 hook 提供当前生产证书，不安装 staging 证书。脚本验证域名、至少七天有效期及公私钥配对，然后原子交换整个 TLS 目录。它校验专属 Nginx master，在该进程的挂载命名空间检查配置，发送 HUP，并通过回环地址的 SNI 握手核对证书指纹。失败时恢复旧目录、重新加载并以非零状态退出。

脚本依赖 Python 3.9 及以上版本、OpenSSL、`nsenter`、glibc 的 `renameat2` 和支持 `RENAME_EXCHANGE` 的 Linux 内核及文件系统。Ubuntu 现有普通 TLS 目录无需迁移为符号链接。若原子交换不受支持，脚本失败且保留原 TLS 目录。每次证书变更后，旧证书和密钥留在 `/etc/lanlink-website/.tls-backup-*`，目录权限为 `0700`。验收后按证书保留策略清理历史备份。重复部署同一证书不会新增备份。
