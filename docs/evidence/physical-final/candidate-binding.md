# 实机候选的构建与校验

| 候选                           | 源码 / 构建                                                    | SHA-256                                                            |
| ------------------------------ | -------------------------------------------------------------- | ------------------------------------------------------------------ |
| Apple Silicon Mac 已安装 Agent | `4e8db4eb445435388b2fd6494bfa9b32796ff479`                     | `bea9e5ce87c6f251f4d5717120cbb05549a658264122e9b4c70f69c975e28547` |
| Apple Silicon Mac PKG          | 同上，本机打包                                                 | `509b1602ea6088db5e504890ed202a1429672a382755b370272a20bf773f8f75` |
| Apple Silicon Mac ZIP          | 同上，本机打包                                                 | `0032c92dd681fb3ca4df9e9ecef662b61a0b4d56e017fd8ac9fac7378fe9e2aa` |
| Windows x64 已安装 EXE         | `fad384cf32187928f260d99efec1bb10de19c873`，CI run 44074931608 | `057db3721886e7d08562eed9560e14d92a431d9051c66bf43160693329570d1e` |
| Windows x64 ZIP                | 同上                                                           | `b2971887164a8fd358a12dbe0f499b54d4555533ff58d19824edaee85e941a46` |
| Intel Mac ZIP                  | `fad384c`，CI run 44074931608                                  | `0de17c0649411793163d104498011e23d7a886c2cddc0c88337cd790f1871259` |

`git diff --name-only 4e8db4e..fad384c` 只列出 `scripts/smoke.py`。两提交的 `crates/` 和 `Cargo.lock` Git 对象均一致。Mac 原生安装运行的是本机打包的应用，Windows 安装的是该 CI 运行的 Windows ZIP。Intel ZIP 经过 CI 构建测试和本机下载哈希校验，物理 Intel Mac 尚无验收记录。[CI run 44074931608](https://github.com/JackEngineer/LanLink/actions/runs/44074931608)

在 Mac 上重新校验已安装二进制、四个构建包及 ZIP 内的 Mac Agent 与 Windows EXE。Mac ZIP 中的 Agent 和已安装 Agent 哈希一致。已安装应用通过 `codesign --verify --deep --strict`，签名为 ad-hoc。原始 Mac 打包日志保留四条 `write: Permission denied`，随后成功产出 PKG 与 ZIP。

[发现回归日志](discovery-regression.log)中的修复版独立发布二进制哈希为 `a58142a311fa00a6b4aa4df55105f282270560dfd238a2046f1e724544e33d13`。上表使用打包并签名后的 Agent 哈希标识实机候选。

受控 IP 迁移测试在当前源码提交 `a1287706f3a87817a2f2676116fcb0416b7b513b` 中加入。与 `fad384c` 相比只修改 `scripts/ip-migration.py` 和 `scripts/smoke.py`，测试使用上表的 Apple Silicon 已安装 Agent。
