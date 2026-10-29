# 路由器修复与本地诊断分支（2026-10-29）

仓库主分支实际名为 `main`，不是 `master`。本次经远端更新确认的基线为 `af617d949eef9875fb5efbb89f2a1abe6894fce6`。以下两个分支均为本地分支，创建时没有推送或部署。

## 待合入的修复

`codex/router-bugfixes-20261029` 直接从该 `main` 基线创建，按问题保留独立提交：

| 提交 | 修复与回归覆盖 |
|---|---|
| `4687e1e` | TCP 发送窗口扣除在途字节，非法 ACK 不得提前清空重传队列 |
| `f9781d5` | 节点组选择共享探测，并限制组内及全局并发 |
| `c2106cb` | SOCKS UDP 已结束任务不再依赖下一个客户端包才能回收 |
| `6e0bc42` | Reality 探测取消时清理在途登记，并可靠唤醒等待者 |
| `ffee1cc` | task-local 上下文包装直接返回 Future，避免嵌套 async 重复保留空间 |
| `f1687e6` | 本地 ARM64 最终链接使用 Rust 自带 musl，核验分配器并防止重走 Zig 最终链接链路 |

该分支保留对应测试、构建指南和必要的诊断依据，不包含可启用的 `memory-profile` 功能、分配归属实验补丁、部署过程笔记或本机现场采集器。编译链修复有目标设备的受控对照证据；真实服务长期内存和慢速是否完全改善仍需后续验证，分支整理不代表它已经上线。

查看待合入内容：

```bash
git log --reverse --oneline origin/main..codex/router-bugfixes-20261029
git diff --stat origin/main...codex/router-bugfixes-20261029
git diff origin/main...codex/router-bugfixes-20261029
```

后续合入或开 PR 使用这个修复分支；可以按主题审阅或选择提交。

## 本地工作与诊断参考

`codex/router-local-diagnostics-20261029` 基于原来的 `codex/tcp-window-ack-fix-20261028`，保留整理前的改进、调查笔记和部署记录，并补存此前只在忽略目录中存在的诊断源码：

- CLI 的可选 `memory-profile` 功能，默认关闭；记录 Rust 请求字节、尺寸桶和有界峰值栈。
- `tools/diagnostics/memory-profile/` 中的离线解析器、分配器滞留复现程序及使用说明。
- 有界 Rust/C 分配归属追踪的实验补丁。该实验不是已验证的线上修复；其原 Zig 链接路径不支持 `--wrap`，不能未经目标验证就用于线上。
- 匿名调查记录及单独运行 vendor 测试时生成的锁文件。

修复分支中的生产代码与参考分支相应文件已逐字节核对一致。两边提交哈希不同，是因为修复分支从干净 `main` 重新拣入，没有带上原分支的部署记录提交。

在工作区无未保存修改时，可切换到参考分支查看或编译诊断功能：

```bash
git switch codex/router-local-diagnostics-20261029
cargo test -p sockrocket-cli --features memory-profile --offline memory_profile::tests
bash tools/build-merlin-musl.sh --features memory-profile
```

仅阅读诊断说明也可以不切换工作区：

```bash
git show codex/router-local-diagnostics-20261029:tools/diagnostics/memory-profile/README.md
```

不要把参考分支整体合入主干来获取修复；后续需要诊断功能时，应单独审查该功能和开销。

## 保留边界

原分支 `codex/tcp-window-ack-fix-20261028` 仍保持在 `67290b6`，未重写历史。`.local/` 中的凭据、配置、原始日志、二进制、符号及私有证据没有进入任一新分支。无关的 `.impeccable/` 本地缓存也未提交。

本地构建的长期维护约束见[构建指南](../merlin-local-build.md)；分配器与 Actions 构建链的区分见[调查记录](allocator-retention-and-ci-build-2026-10-29.md)。

## 整理后验证

修复分支完整 core/CLI 回归：676 项通过、0 失败、15 忽略。参考分支的 memory-profile 三项分配器单元测试通过，实验覆盖补丁通过 `git apply --check`，离线解析器使用匹配的 ARM 符号实际解析通过。整理前29个非缓存修改或未跟踪文件均已按SHA256核对保存在参考分支；两分支的修复源码与构建工具一致。
