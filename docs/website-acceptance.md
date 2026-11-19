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

## 安装包构建来源验证

2026-11-18，腾讯云 Ubuntu 上的同步脚本、固定工作流副本和 systemd 单元经 SHA-256 比对，与 PR #12 合并提交 `be45f01` 一致。服务器使用 GitHub CLI 2.97.0，验签程序、工作流副本和 Sigstore 信任根均通过 root 所有权与权限检查。生产定时器 `lanlink-release-sync.timer` 每 15 分钟运行，核查开始时已启用。 [合并提交](https://github.com/JackEngineer/LanLink/commit/be45f01ad0f244fc1dd5fe14ca52d07046c45fc3) · [部署与回滚说明](../deploy/release-sync/README.md)

定时器启用后，使用生产账号 `lanlink-release` 在隔离的 systemd 服务中调用同步程序的检查模式。测试使用临时标签 `v0.2.0-preview.0` 的真实构建产物，现场校验 GitHub 的标签、提交、工作流身份和签名。四个原始安装包全部通过。缺少签名证明、篡改校验清单、篡改安装包三种情况均被拒绝。隔离检查从本地暂存目录读取构建产物，生产站点目录以只读方式挂载。测试标签没有公开 Release，验收后已删除。 [隔离测试的签名构建](https://github.com/JackEngineer/LanLink/actions/runs/44257273942)

## v0.2.0-preview.3 公网验收

2026-11-18 15:27（北京时间），首个带构建来源证明的预览版在 GitHub 公开。Release 提供四个安装包、`SHA256SUMS` 和 `SHA256SUMS.sigstore.jsonl`。 [GitHub Release](https://github.com/JackEngineer/LanLink/releases/tag/v0.2.0-preview.3) · [签名构建](https://github.com/JackEngineer/LanLink/actions/runs/44256691714)

生产定时器于 15:30 开始同步，暂存期间官网继续提供 `v0.2.0-preview.2`，新版直链返回 404。Apple Silicon PKG 和 Intel Mac ZIP 的 CDN 传输中断后，从同一 GitHub Release 取得这两个文件，核对签名清单后补入私有缓存。生产同步服务在重试时重新验签、校验四个包，于 15:53 切换官网入口。

切换后，公网 HTTPS 首页指向 `v0.2.0-preview.3`。公网 `SHA256SUMS` 再次通过来源验签，且 SHA-256 与 GitHub Release 一致。从官网完整下载了以下四个安装包，HTTP 状态均为 200，文件大小与 Release 一致，SHA-256 与签名清单及 Release 一致。 [官网下载](https://lanlink.heyyami.com/#download) · [官网 SHA256SUMS](https://lanlink.heyyami.com/downloads/v0.2.0-preview.3/SHA256SUMS)

| 安装包                                     |   字节数 | SHA-256                                                            |
| ------------------------------------------ | -------: | ------------------------------------------------------------------ |
| `LANLink-0.2.0-aarch64-apple-darwin.pkg`   |  6045014 | `43bc97735462dc68015a573b7006aee6bf5f72f015ffc06d9963f475d01b6fe4` |
| `LANLink-0.2.0-aarch64-apple-darwin.zip`   | 12089149 | `5a37bc3ffbdaa838939a1dff5c608b186f9e565cd6ab2fa56d0468a6a65f478b` |
| `LANLink-0.2.0-x86_64-apple-darwin.zip`    | 12702932 | `e2b70b76312de52fc9e88d0a6c3a9d9c027341a8aa34a8323dc95d1cf6c9f82e` |
| `LANLink-0.2.0-x86_64-pc-windows-msvc.zip` |  4882423 | `0ce42f575474b31aba8932bcec6e983c610d0630a9fcbce175ffb8a641bbeb56` |

`v0.1.0-preview.1`、`v0.2.0-preview.1` 和 `v0.2.0-preview.2` 共 15 条历史安装包与校验清单直链均返回 HTTP 200。旧安装包可通过 HTTP 范围请求读取，文件大小和三个版本的校验清单与切换前一致。

## 安装包断点续传验收

2026-11-18 16:14:27（北京时间），腾讯云完成首次隔离演练。故意中断真实 GitHub CDN 的响应后，新进程从私有缓存保留的 5,983,612 字节偏移继续下载。四个包通过来源与 SHA-256 校验后，才切换隔离站点的下载入口。 [PR #13](https://github.com/JackEngineer/LanLink/pull/13)

16:15:26，腾讯云切换生产同步脚本。随后核对线上脚本的 SHA-256，确认与 PR #13 的合并提交 `2c28d2b` 一致。服务器保留了切换前的脚本备份。16:45 的生产定时任务退出码为 0，`lanlink-release-sync.timer` 处于启用和运行状态。 [合并提交](https://github.com/JackEngineer/LanLink/commit/2c28d2b6adf14b3ec811fe83a996d7cf4c8984ce)

使用合并版脚本复跑时，以生产账号 `lanlink-release` 在隔离的 systemd 服务中读取 `v0.2.0-preview.3` 的签名清单和真实 CDN 资产，生产站点目录以只读方式挂载。故意中断 Apple Silicon PKG 的响应后，私有文件保留了 5,982,234 字节。新进程发送 `Range: bytes=5982234-`，收到 HTTP 206。四个安装包通过完整 SHA-256 校验后，隔离站点切换到 `v0.2.0-preview.3`，三个历史版本的文件逐字节一致。

公网复验时，官网仍提供 `v0.2.0-preview.3`，首页内容与核查前一致。四个现行安装包均从官网完整下载，文件大小与 GitHub Release 一致，SHA-256 与签名清单及 Release 一致。现行和历史版本共 20 条安装包与校验清单链接均返回 HTTP 200。安装包通过了范围读取检查，四份公开校验清单均与核查前一致。服务器上的四个版本共 16 个安装包也通过了 SHA-256 校验。 [官网下载](https://lanlink.heyyami.com/#download)

## 新版官网部署与验收

2026-11-18 晚（北京时间），以 PR #18 的合并提交 `2ee4310` 构建并发布新版官网。部署包 SHA-256 为 `2a9030f015c9470a4c63daab49c1aa94840bc59fbf687d2dc40e78bf0c04dd08`，上传后在服务器复验一致。下载区提供 `v0.2.0-preview.3`。 [PR #18](https://github.com/JackEngineer/LanLink/pull/18) · [合并提交](https://github.com/JackEngineer/LanLink/commit/2ee4310198ecd03ef9c890c717ee571cea21a272)

在安装包同步锁内复制四个版本的下载目录，逐文件比对后按各版 `SHA256SUMS` 校验 16 个安装包。Nginx 配置检查通过后，原子切换 `/var/www/lanlink/current`。切换后用 HTTPS 回环请求核对首页、主要素材和现行版校验清单，确认新目录已生效。

- 公网首页、CSS 和图片共 11 个文件均返回 HTTP 200，MIME 类型正确，响应内容与部署包逐字节一致。首页保留安全响应头，HTTP 跳转至 HTTPS，未知路径返回 404。同机主站 `heyyami.com` 返回 HTTPS 200。
- 浏览器实测电脑互传、手机扫码、局域网聊天三种演示切换，以及扫码步骤展开、截图放大和关闭、下载区跳转。下载区有三个平台的四个安装包入口。
- 四个版本的安装包及校验清单共 20 条公网直链均返回 HTTP 200。完整下载的四个现行安装包均通过 SHA-256 校验。Apple Silicon PKG 请求 `Range: bytes=1024-2047`，返回 HTTP 206 和 1,024 字节。

发布目录为 `/var/www/lanlink/releases/website-2ee4310`。回滚目标为部署前的 `/var/www/lanlink/releases/v0.2.0-preview.3-bxnb6oj4`。回滚前核对现行下载版本和历史下载目录，保留新增的公开直链，再按部署指南原子切换 `current` 并复验。 [回滚步骤](website-deploy.md#5-回滚)

## v0.2.0 正式发布与公网验收

2026-11-19 02:35:55（北京时间），GitHub 公开发布 `v0.2.0`，并将其标为最新正式版。标签指向 PR #20 的合并提交 `9cfa73a`，与 `v0.2.0-preview.5` 使用同一源码提交。发行工作流完成 Apple 芯片 Mac、Intel Mac 和 Windows x64 的测试与打包。Release 提供四个安装包、`SHA256SUMS` 和 `SHA256SUMS.sigstore.jsonl`，共六个资产。[正式版 Release](https://github.com/JackEngineer/LanLink/releases/tag/v0.2.0) · [签名构建](https://github.com/JackEngineer/LanLink/actions/runs/44367929423) · [合并提交](https://github.com/JackEngineer/LanLink/commit/9cfa73ae0af06309123f346756248bab06957947)

将已核对的原始安装包预置到同步服务的私有缓存。现有服务重新验证标签、工作流、Sigstore 构建来源证明和包摘要后切换官网。公网首页显示「正式发布 v0.2.0」，`/release.json` 返回 `{"tag":"v0.2.0"}`。官网校验清单与发行工作流产物逐字节一致。[官网下载](https://lanlink.heyyami.com/#download) · [版本索引](https://lanlink.heyyami.com/release.json) · [校验清单](https://lanlink.heyyami.com/downloads/v0.2.0/SHA256SUMS)

从公网完整下载四个正式安装包，SHA-256 均与签名清单一致。

| 安装包                                     | SHA-256                                                            |
| ------------------------------------------ | ------------------------------------------------------------------ |
| `LANLink-0.2.0-aarch64-apple-darwin.pkg`   | `cf35e9ca4a59abf571df2e194e2074a63b7e2dde2ec77ac4164f89dceb323381` |
| `LANLink-0.2.0-aarch64-apple-darwin.zip`   | `900f7e46fa01af7e2dedbb35e0e5ac928a6f4ed575e98634ab0981f388e95119` |
| `LANLink-0.2.0-x86_64-apple-darwin.zip`    | `582acec83188e390d710fab25d57c554cc37d7571875f51795b646a46cd3cb89` |
| `LANLink-0.2.0-x86_64-pc-windows-msvc.zip` | `1422013e38b55a995991fce0ae2736c97793f424d969d5a7ba71d7630bfa81e1` |

六个历史预览版本的校验清单均可获取，24 条历史安装包直链均返回 HTTP 200。Mac mini 和 Windows 从官网 `v0.2.0-preview.5` 升级的结果见[一键升级实机验收](acceptance.md#v020-一键升级实机验收)。

## AI 助手安装入口公网验收

2026-11-19（北京时间），从公网访问官网下载区。首页、安装说明、版本索引等 13 个静态文件均返回 HTTP 200，内容与 `8e85a34` 的官网构建产物逐字节一致。首页实际返回的 CSP 与仓库 Nginx 配置一致。[官网下载区](https://lanlink.heyyami.com/#download) · [安装说明](https://lanlink.heyyami.com/install.txt) · [功能 PR #23](https://github.com/JackEngineer/LanLink/pull/23)

- 桌面 Chrome 默认显示「通过 AI Agent 安装」。点击「复制」后，从系统剪贴板读回的文字与页面指令完全一致：`请按 https://lanlink.heyyami.com/install.txt 安装并打开 LANLink。`
- 在 375 像素宽的 Chrome 视口模拟剪贴板写入被拒绝，页面提示手动复制。关闭 JavaScript 后仍可查看指令并切换到手动安装。桌面和窄屏均可看到四个手动下载入口，验收期间未观察到页面或控制台错误。
- 从官网完整下载 Apple Silicon 的 PKG 和 ZIP、Intel Mac 的 ZIP、Windows x64 的 ZIP。四个文件的 SHA-256 均与 GitHub 正式版清单及上表一致。本次验证覆盖下载与文件完整性，未重新运行平台安装程序。[v0.2.0 正式版](https://github.com/JackEngineer/LanLink/releases/tag/v0.2.0)

## Mac 与 Windows Agent 执行官网安装指令

2026-11-19（北京时间），分别将官网下载区复制出的原文“请按 https://lanlink.heyyami.com/install.txt 安装并打开 LANLink。”交给 Apple Silicon Mac 和 Windows x64 上的 Agent。两台电脑均已有 LANLink，按官网说明先尝试打开既有应用。[官网安装指令](https://lanlink.heyyami.com/install.txt)

| 环境                        | 初始状态                                               | 执行和观察                                                                                                                                                                                                                                                                                                      |
| --------------------------- | ------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS 27.0（26A428），arm64 | `/Applications/LANLink.app` 已安装 0.2.0，服务正在运行 | 调用 `open /Applications/LANLink.app` 返回 0。启动日志记录重复实例的数据目录锁错误，原服务继续响应。`/api/v1/health` 返回 `ok` 和版本 `0.2.0`。用独立 Chrome 主动打开回环首页，HTTP 200，可见「设备」「聊天」「文件」「设置」，点击「设置」显示 0.2.0。未观察到启动器自动打开标签页。控制台仅报告 favicon 404。 |
| Windows 10 22H2，x64        | `%LOCALAPPDATA%\Programs\LANLink` 已安装 0.1.0         | 在现行 Windows PowerShell 策略 `Restricted` 下运行既有 `Launch.ps1`，收到 `UnauthorizedAccess`，退出码 1。回环端口 38761 无监听，界面未打开。Agent 在此停止，未使用旧快捷方式中的策略绕过参数。                                                                                                                 |

两台机器均未重装或升级，均未发现现成的干净测试虚拟机。以上结果覆盖已有安装；从空白环境下载、安装到首次打开的完整流程仍待验证。

### Mac mini 保留数据卸载后重装

2026-11-20（北京时间），在上述 Apple Silicon Mac mini 上执行官方卸载脚本，停止服务。备份默认数据目录并逐文件比对后，将旧应用包移出安装目录，保留作恢复用。Agent 开始执行官网原指令时，系统和用户的 Applications 目录均无 LANLink，回环端口 38761 无监听。

1. Agent 核对官网和 GitHub 最新正式版均为 `v0.2.0`。通过 Chrome 从官网下载 Apple Silicon ZIP，大小为 14,265,454 字节，SHA-256 为 `900f7e46fa01af7e2dedbb35e0e5ac928a6f4ed575e98634ab0981f388e95119`，与该版 GitHub Release 清单一致。[正式版 Release](https://github.com/JackEngineer/LanLink/releases/tag/v0.2.0)
2. 使用 macOS 归档实用工具解压，将应用放入 `/Applications/LANLink.app`。安装后的版本为 `0.2.0`，保留了浏览器下载的隔离属性。
3. 正常调用 `open /Applications/LANLink.app` 返回 0。Gatekeeper 的执行评估返回 `rejected`（退出码 3），端口 38761 无监听，健康接口连接失败，未观察到可用界面。Agent 按官网说明停止，未移除隔离标记或修改安全设置。
4. 重装后复核原数据库、接收文件和其余持久文件，内容均与停机备份一致。原库与备份均通过 SQLite `quick_check`。比对排除了 SQLite 的共享内存及预写日志辅助文件，备份中的预写日志为 0 字节。

本轮实测结果为下载、校验、安装及用户数据保留通过，重装后的首次打开未通过。该轮保留了既有用户数据；Windows 验收结果见上表。
