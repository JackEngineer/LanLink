# 官网安装包自动发布

只有 `.github/workflows/release.yml` 为对应标签构建并签名的发行文件才能更新腾讯云 Nginx 首页。已有版本及 `/downloads/<tag>/` 直链继续保留。

## 构建和发布 GitHub Release

1. 将审核后的源代码合入 `main`，确保 `package.json`、`apps/web/package.json` 和 Cargo workspace 的版本一致。
2. 推送 `vX.Y.Z` 或 `vX.Y.Z-preview.N` 标签；标签基础版本必须等于源代码版本。工作流会先运行站点与同步测试，再在 GitHub 托管的 macOS ARM、macOS Intel、Windows runner 上测试并打包。
3. 在该标签的 **Build verified release** 运行中，下载唯一的 `release-set-<tag>` artifact，解压后把其中六个文件原样上传到同一标签的 GitHub Release 草稿：四个最终安装包、`SHA256SUMS`、`SHA256SUMS.sigstore.jsonl`。不要上传 artifact 外层 ZIP，不要重新压缩或修改文件。工作流不自动发布 Release。预览标签应勾选预发行版本，全部文件上传完后再发布。

最终安装包名称为 `LANLink-X.Y.Z-aarch64-apple-darwin.pkg`、`LANLink-X.Y.Z-aarch64-apple-darwin.zip`、`LANLink-X.Y.Z-x86_64-apple-darwin.zip`、`LANLink-X.Y.Z-x86_64-pc-windows-msvc.zip`。独立的 Ubuntu 签名任务只读取本次运行的构建 artifact，核对恰好这四个文件，计算 `SHA256SUMS`，再使用固定提交的官方 `actions/attest` 为清单生成 SLSA 构建来源证明。清单的签名与四个文件的 SHA-256 一起覆盖全部安装包字节。

服务器每 15 分钟检查公开 Release。每个新版本必须依次通过以下检查，`--check`、缓存命中及回滚目录内已有安装包也不例外：

- 通过 GitHub Git API 解析精确 `refs/tags/<tag>`（支持 annotated tag），确认提交在 `main` 历史中；不使用 Release 的 `target_commitish`。
- 对比该提交中 `release.yml` 的不可变 Git blob 摘要和 root 安装的审核副本。仅有同名工作流不足以获得信任，标签中的替换工作流会被拒绝。
- 下载有大小上限的清单与 Sigstore bundle，通过 root 安装的 GitHub CLI 验证签名、仓库、完整工作流标签身份、源 ref、源提交及签名工作流提交，拒绝自托管 runner。使用本地信任根和隔离的空 CLI 配置，无需 GitHub 令牌或登录。
- 确认签名清单恰好覆盖四个文件，核对 GitHub 的文件大小及可用摘要，并重新读取暂存包校验 SHA-256。切换前重新解析标签，标签移动则停止。

来源证明缺失、无效、错误标签或提交、非审核工作流、缺包、校验失败或暂存出错时保留在线版本，定时器下次重试。当前已在线的历史版本不追补来源证明；没有新品时仍检查已有文件和公网入口，避免中断旧链接。Sigstore bundle 仅用于临时校验，不增加首页下载链接。

同一轮发现多个新品时先验证整批，再一次切换首页和下载目录。首页选择最高版本，整批版本直链均可用。单轮最多运行两小时；上一轮仍在同步时不会启动第二轮。单文件传输中断会独立重试，优先使用 GitHub 资产 API，必要时改用该 Release 官方直链。通过校验的安装包暂存于站点之外的私有缓存，后续运行必须先重新验证来源证明才可重新校验并复用缓存，发布后清理该版本缓存。低于当前官网版本的补发标签不自动导入。

安装包未完成的字节也保存在私有缓存中，按标签、签名清单 SHA-256、文件大小及 GitHub 资产 ID 隔离。每次重试都从官方入口重新获取下载响应，并用 `Range` 继续；定时器下次运行及进程重启后仍可续传。只有严格匹配偏移、总大小及 identity 编码的 `206` 响应才追加；服务器忽略 Range 时单独重下，未取得更多字节不会覆盖原有进度。单包每轮最多九次请求、二十分钟，空闲读取超时三十秒。损坏的完整文件会清空重试，完整文件必须重新读取并通过已验证签名清单的 SHA-256 校验后才能进入可复用缓存和网站暂存目录；任何失败都保持旧首页及下载直链。

新目录继承所有历史下载路径。历史包优先使用硬链接，系统拒绝时复制文件。历史文件应保持只读，手动官网部署也须保留全部版本下载目录。首次同步到新版时会生成 `/release.json`，并与新版首页及安装包一起切换。切换后检查公网首页、版本索引、校验清单和安装包 HTTP 大小。公网检查失败会记日志，下一次运行复查当前版本。已开始分发的直链继续保留。

## 在现有 Ubuntu 部署一次

要求 Python 3.10+、支持上述验证参数的 GitHub CLI（已验证 `gh 2.97.0`，包括 `--source-ref`、`--source-digest`、`--signer-digest`、`--deny-self-hosted-runners`、`--custom-trusted-root`）。按 [GitHub CLI 官方安装说明](https://github.com/cli/cli/blob/trunk/docs/install_linux.md) 验证并安装 `/usr/bin/gh`；不能使用同步账号可替换的二进制或包装脚本。CLI、信任配置及其全部父目录必须为 root 所有、不可由组或其他用户写入，配置文件不允许符号链接。

现有 `/var/www/lanlink/current` 应指向 `releases` 内目录，文件允许部署账号及 Nginx 读取。以下从已审核、即将合入 `main` 的源码安装固定工作流副本。**不要从待发布标签或 Release 下载信任配置**。部署脚本、固定工作流或信任根变更均需要管理员从审核后的来源重新安装；自动同步不更新这些信任锚。工作流副本不同的新标签会保持旧版在线并报错，直到审核并更新副本。

来源证明把安装包绑定到 `main` 历史中的版本提交和批准的构建流程。能够修改 `main` 的账号仍处于源码信任边界内。若需限制发布凭据写入未经审核的源码，应在仓库层为 `main` 要求独立审核，并限制绕过权限。

```sh
id lanlink-release >/dev/null 2>&1 || sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin lanlink-release
sudo chown lanlink-release:lanlink-release /var/www/lanlink /var/www/lanlink/releases
sudo install -d -o root -g root -m 0755 /usr/local/libexec /etc/lanlink-release-sync
sudo install -o root -g root -m 0755 deploy/release-sync/sync.py /usr/local/libexec/lanlink-release-sync.py
sudo install -o root -g root -m 0644 .github/workflows/release.yml /etc/lanlink-release-sync/release-workflow.yml
# 获取官方 Sigstore 信任根快照，无需 GitHub 登录；先写临时文件，成功后再安装。
trust_snapshot=$(mktemp)
/usr/bin/gh attestation trusted-root > "$trust_snapshot" && sudo install -o root -g root -m 0644 "$trust_snapshot" /etc/lanlink-release-sync/trusted-root.jsonl
rm -f "$trust_snapshot"
sudo install -o root -g root -m 0644 deploy/release-sync/lanlink-release-sync.service /etc/systemd/system/lanlink-release-sync.service
sudo install -o root -g root -m 0644 deploy/release-sync/lanlink-release-sync.timer /etc/systemd/system/lanlink-release-sync.timer
sudo systemctl daemon-reload
sudo -u lanlink-release /usr/bin/python3 -B /usr/local/libexec/lanlink-release-sync.py --check
sudo systemctl start lanlink-release-sync.service
sudo systemctl enable --now lanlink-release-sync.timer
sudo systemctl list-timers lanlink-release-sync.timer
```

信任根快照在验证期间不联网更新，也不写系统状态。Sigstore 轮换时，管理员按上述命令重新获取官方快照并安装，然后运行 `--check`；不要通过跳过来源验证恢复发布。[GitHub CLI 验证参数](https://cli.github.com/manual/gh_attestation_verify) 和 [官方 attestation action](https://github.com/actions/attest) 说明了身份约束和 bundle 格式。

`--check` 在暂存目录验证新品后清理暂存文件，不切换当前版本；若最新版本已经在线，检查现有文件及公网链接。用 `sudo journalctl -u lanlink-release-sync.service -n 100 --no-pager` 查看日志，用 `sudo systemctl stop lanlink-release-sync.timer` 暂停，用 `sudo systemctl start lanlink-release-sync.service` 重试。异常关机残留隐藏 `.sync-*` 目录时，停止服务和定时器，确认目录不在 `current` 路径上后再清理。

手动回滚前先停用定时器，并确认正在运行的任务已结束。把已公开版本下载目录保留在回滚后的站点中。重新启用后，服务仍须先验证最新版本来源证明，再校验已有文件并恢复最新下载入口。

### macOS 正式版签名与公证

正式标签的 macOS 作业需要同一 Apple Developer 团队的 Developer ID Application 和 Developer ID Installer 证书及各自的私钥，以及可提交公证的 App Store Connect 团队 API 密钥。发布负责人在 GitHub 仓库设置中配置以下值。

| 类型              | 名称                                                                 | 内容                                                |
| ----------------- | -------------------------------------------------------------------- | --------------------------------------------------- |
| Actions variables | `MAC_APP_IDENTITY`、`MAC_INSTALLER_IDENTITY`                         | 两个签名身份的完整名称，包括团队 ID                 |
| Actions secrets   | `MAC_CERTIFICATES_P12_BASE64`、`MAC_CERTIFICATES_PASSWORD`           | 包含两个身份的加密 PKCS#12 的 Base64 内容及导出密码 |
| Actions secrets   | `MAC_NOTARY_KEY_BASE64`、`MAC_NOTARY_KEY_ID`、`MAC_NOTARY_ISSUER_ID` | 公证团队 API 私钥的 Base64 内容、Key ID、Issuer ID  |

仅在受信任的发布提交上创建正式标签。凭证准备、签名或公证失败时，工作流会停止，不会生成可同步的正式发行包。预览标签继续使用开发打包路径。更新本次工作流后，须先按本手册审核服务器上的工作流副本，再发布新版本并保留旧下载链接。

上线后，在启用 Gatekeeper 的 Mac 上从官网浏览器重新下载，对照该版 GitHub 清单校验 SHA-256，解压并安装。记录 Gatekeeper 评估、首次打开、本机健康接口、界面版本及用户数据比对结果。[Apple 公证要求](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
