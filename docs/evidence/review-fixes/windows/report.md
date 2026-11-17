# Windows c076be2 原生验收与93a565d LICENSE包

2026-11-17。Windows10 22H2 build19045.6466。生产程序候选c076be2e17670a27791d00eb5b46ff90322dd551；分发元数据候选93a565d4693f5574da3493320068a890ec611138。两者crates/apps无差异。

## 原生构建与安装

项目隔离Rust1.100.0、Node24.22.3、npm11.19.1，已有MSVC14.44.35207、WindowsSDK10.0.26100.0。Rust和Node下载来自官方站并核对官方SHA256，未改全局PATH。npm ci、npm build通过。c076原生24 Web测试、lint、34 Rust测试、严格Clippy及fmt全通过，原scripts/windows/package.ps1成功。
原ZIP SHA256 1b2be96360c7227339e9cc696af8b6e70c412ec9e5acd162873b01af2497ec7f，EXE SHA256 08aeb1733f97acea4ddac9529ab29f968731eb811efc5dbfd27b30e0d4ddfa71。
升级后PID30696、health ok、原身份和配对保留，逐项核对10条消息和18条传输记录无变化，登录启动关闭。

## 多文件与上限

Mac新版网页一次选择两个64MiB文件，Windows原生收到两项offer。alpha17dcec89-8b61-4397-ac32-950c7c25a683接受后completed，实际落盘SHA281e519df3077b557c6b03f5da83c4e8d397219259615dd7c3308f89cae8f2a6匹配。beta bae1e8e8-785c-4fff-a2e2-3ae8a8c316cf保持待接收，降低Windows上限1MiB后变rejected，未发布完整文件，alpha哈希未变。

初次Windows也设1MiB时上传10MiB，测试客户端报告HTTP状态未取得、传输连接被中止，没有transfer ID。这是本地上限拒绝，不计为远端拒收通过。
纠正顺序，Windows2GiB、Mac1MiB，发送acdc7ab4-bc14-442d-9fa6-c90fb9b7c5f6后Windows记录rejected/offset0；中文消息5fb6443b-c720-4085-81aa-064dec730517为sent。Mac协调端独立确认无接收记录或文件。
反向Mac2GiB、Windows1MiB，Mac探针fe3a146c-7608-4c13-8f28-940854382031在Windows无记录、无part、无完整文件。消息f892529c-f79c-4624-85f9-68d6b9ab2702唯一接收sent。Windows随后恢复2147483648并GET回读。
一次日志筛选空输出未建文件，后续Get-Content失败。已保留说明，修正只读计数为0。不能以无日志独立证明无发送循环。

## 双向1GiB和实际中断

Windows发送bcbed60c-1ac8-42fb-b5de-2413377f6ee2。Mac强杀接收端后，双方记录4194304持久偏移，Mac part同长。Windows PID30696始终不变，自动恢复至completed。Mac独立落盘SHA9674bda70c01181b475879a0bd0ea2847d55335fb592fff65f423f4e36d5d859匹配。Windows此轮继承RUST_LOG=warn，没有首帧发送offset日志，不将旧候选证据替代。
反向任务5298ba91-b7f8-4bf8-a960-5616578afe56，Windows在UTC12:41:32.570强停PID30696。停止后SQLite offset341835776、part341835776、完整目标不存在。最后API340787200与停止时刻有1MiB差异，采用停止后持久值。
Windows于12:42:42.271显式RUST_LOG=lanlink_agent=info启动相同程序，新PID31824。Mac同一发送进程日志12:42:43.358406Z首发offset341835776，与停机值一致，该原始日志由Mac端归档。Windows自动completed，实际落盘1073741824字节、SHA9abc035488a85bdd2237e51ed32481c8bba1a326e2730145d1ac1971d52ff19e匹配。
accept有一次Agent错误body only incoming offers can be accepted，退出1。该次未保存HTTP状态码，源码映射推定400，不能称现场捕获。紧随回读已经transferring，与已接受状态下再次accept冲突一致；首次接受来源未确定，不把该错误计为成功或未经证明判为产品缺陷。具体命令/原body/限制见runtime/reverse-accept-failure-analysis.md。

## 含LICENSE的新包

从 c076be2 到 93a565d，8fe8bc8 增加专项回归脚本，93a565d 让打包脚本复制 LICENSE。重新执行原 package 脚本，包 SHA-256 为 308ba729fd73a45594b8f52d6ba32b028942b15d839470d31a2dc7a3e1d22f55。EXE 仍为 08aeb1733f97acea4ddac9529ab29f968731eb811efc5dbfd27b30e0d4ddfa71。
包中EXE、3个Web文件、3个安装脚本与已验收安装逐字节相同，共7文件、差异0。LICENSE与仓库根LICENSE一致，仅补到现有安装目录，无重启。安装额外保留旧的未引用web/assets/index-D33PEGC8.js，新包不包含，未删除。包已通过LANLink提交，ID791fdcc5-4afd-4bc2-8e4a-350af8a4e878。

最终PID31824、上限2GiB、登录启动关闭。保留旧包、数据和配对。此包不含私钥、session token、数据库或大测试文件；仅SQLite中指定测试transfer记录的脱敏摘录。原始完整Agent日志保留本机。真实文件验收不代替设备断电或公开发行签名验收。

归档修正说明：最初证据打包尝试读取不存在的fmt.log产生归档脚本错误。fmt原检查无输出且退出0，已由checks-passed.json确认。本最终包改用fmt-result.txt说明，无重跑测试、无伪造日志。
