# 路由器匿名内存滞留与编译链核查（2026-10-29）

可执行的构建步骤和后续维护约束见[Merlin 本地构建指南](../merlin-local-build.md)。本文保留此次诊断与验证证据。

## 已定位的机制

两台路由器当前 MD5 `91167aa902392f879024564b6a88bf65` 的本地构建包含 Zig 0.16 的 `c.malloc` / `heap.SmpAllocator`。原始符号版中 `malloc`（0x1e49b18）、`heap.SmpAllocator.alloc`（0x1e49dc8）及 `free`（0x1e4a1a4），通过对应符号解析分别落到 `/opt/homebrew/Cellar/zig/0.16.0_1/lib/zig/c/malloc.zig:89`、`std/heap/SmpAllocator.zig:118`、`c/malloc.zig:159`。

`c/malloc.zig` 为 malloc 选择 SmpAllocator。SmpAllocator 的 `free`（200–215 行）仅对大块调用 PageAllocator.unmap；小块写回分尺寸、分线程槽位的空闲链表，没有归还整个 slab 的实现。不同尺寸先后出现的峰值会留下多组驻留 slab，Rust 请求字节统计在释放后会下降，RSS仍可保持较高。这是已核实的分配器保留机制；此前将底层实现笼统称为musl不准确。

## 设备最小复现

在用户 RT-AX86U Pro 上运行两个独立的 AArch64 程序，同一份 Rust 源码、同样优化等级，唯一构建变量为最终链接链路。测试不读取路由器配置，不建立网络连接，不替换服务。每轮按一种尺寸（64、128、256、512、1024、2048、4096、8192、16384 字节）分配总共1 MiB、写入、全部释放，然后进入下一尺寸。另经Rust System接口复核，最后进行独立4 MiB大块分配释放。

| 同一工作负载 | Zig 0.16 最终链接 | Rust工具链自带musl最终链接 |
|---|---:|---:|
| 小块阶段测试请求峰值 | 1 MiB | 1 MiB |
| 小块全部释放后RSS | 17656 KiB（17.24 MiB） | 512 KiB（0.50 MiB） |
| 最终所有测试块释放、等待后RSS | 17756 KiB（17.34 MiB） | 512 KiB（0.50 MiB） |
| 测试块剩余请求字节 | 0 | 0 |

这里的0仅指受控测试块，程序自己的标准库和指针数组仍可能持有少量内存。RSS是内核采样，不把首条冷启动RSS当作精确基线。测试前后在线服务PID均为4118，未重启代理；独立测试文件已清理。匿名数值及程序SHA256保存在`.local/memory-attribution-20261029/allocator-repro-anonymous.json`。

该复现能够解释“总存活请求峰值不大、先后经历多个尺寸峰值后匿名RSS明显较大”的机制。尚不能据此断言现场全部361 MiB增长及速度慢都仅有这一原因，后续需要修正构建后的同PID数据验证。

## GitHub Actions 与本地构建不同

核对了`.github/workflows/ci.yml`、`build-packages.yml`、`merlin-package.yml`和`release.yml`，Merlin目标均使用cross，不调用本地`tools/zig-cc-aarch64-musl`作为最终链接器。

并读取了正式Release运行40721374796、ARM64任务122934012624（提交af617d949eef9875fb5efbb89f2a1abe6894fce6）的实际日志，确认：

- cross版本0.2.5；
- 容器`ghcr.io/cross-rs/aarch64-unknown-linux-musl:0.2.5`；
- 镜像digest `sha256:702154f52b2d8091671aa2c84d5582d849f949977228c735ff8462f93cc0e1e4`；
- 命令`cross build --package sockrocket-cli --release --target aarch64-unknown-linux-musl`；
- 最终链接命令明确使用`aarch64-linux-musl-gcc`，并带Rust工具链self-contained目录下的crt对象。

[对应Actions任务](https://github.com/sockrockets/sockrocket/actions/runs/40721374796/job/122934012624)。因此本次已确认的问题来自现场本地候选的Zig最终链接链路，不能直接泛化为Actions正式发布包也存在相同分配器问题。正式CI工作流无需改成Zig；本地构建需在分配器选择上对齐，并核验产物。核查过程未修改或触发Actions。

## 已准备的本地修正

新增`tools/build-merlin-musl.sh`：C依赖继续由Zig交叉编译，最终使用Rust自带rust-lld和musl，显式`link-self-contained=yes`。这与Actions的gcc命令不同，但都避免由本地Zig提供最终malloc/free。保留未剥离符号供审计，再输出剥离版。

新增`tools/verify-merlin-allocator.py`：读取真实ELF符号表，拒绝SmpAllocator，要求malloc/free/realloc以及musl内部实现符号存在；剥离后无法证明归属的文件也拒绝作为审计输入。已验证旧Zig二进制失败、新musl符号版通过、剥离版因缺少证据失败。

最终候选直接沿用已部署快照`.local/customer-async-frame-review-20261029/source`，没有叠加工作区其他修改。候选SHA256 `1fffea8e9ee5fba4dedd2ea049a49dae15a529011fab5a0e7aa43b1db3d379ee`；分配器符号已核验无SmpAllocator、存在musl实现。完整core/CLI、启用memory-profile的本地回归679通过、0失败、15忽略。候选独立ARM分配测试峰值16797450字节，释放后存活20234字节，调用栈已按匹配符号解析到源码；服务PID、配置校验及DNS53/5300均通过。

该候选尚未替换任何在线服务。原先开发的有界Rust/C归属采样仅为隔离目录中的诊断原型；Zig最终链接器不支持GNU --wrap参数，未强行上线该原型，也未将其685项本地测试结果冒充最终候选679项回归。最终候选不包含这些额外钩子。

完整来源与验证清单：`.local/memory-attribution-20261029/provenance.json`。
