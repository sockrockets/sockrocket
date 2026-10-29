# Merlin ARM64 本地构建：链接器与分配器约束

本地 ARM64 构建统一使用 `bash tools/build-merlin-musl.sh`。Zig 只用于编译 C 依赖及归档；Rust 可执行文件最终由 Rust 工具链自带的 `rust-lld` 链接，显式启用 `link-self-contained=yes`，使用该工具链自带的 musl。

不要将 `tools/zig-cc-aarch64-musl` 设置为 `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER`，也不要沿用旧记录里的 `-C link-self-contained=no`。`aarch64-unknown-linux-musl` 目标名、静态 ELF、启动成功和功能测试通过，都不能单独证明最终使用了预期分配器。

旧 C 包装器会拒绝直接带入 Rust `.rlib` 的链接调用，并提示标准构建入口；C 编译、预处理和版本探测仍正常转发。这道入口保护针对已经出现的调用方式，不能替代最终 ELF 分配器校验。

## 为什么有这项约束

2026-10-29 核实：现场本地候选的 `malloc/free` 来自 Zig 0.16 的 `c.malloc`，底层是 `heap.SmpAllocator`。小块释放进入按尺寸、线程槽位组织的空闲链表，没有归还整个小块 slab 的实现。先后经历不同尺寸的峰值后，Rust 存活请求可以下降，匿名驻留内存仍然较高。

同一台路由器、同一份受控测试源码，每轮最多持有 1 MiB 小块、每轮全部释放后进入下一尺寸；小块阶段结束时，Zig 链路 RSS 为 17656 KiB，而使用 Rust 自带 musl 的链路为 512 KiB。现场全部内存增长及速度问题是否均由此引起，仍需上线后的同进程数据验证。这个复现确认的是分配器保留机制，不是所有历史问题的唯一根因。

实际 Actions Release 使用 `cross 0.2.5` 容器及 `aarch64-linux-musl-gcc` 最终链接，并带 Rust self-contained CRT。它与出问题的本地 Zig 最终链接链路不同；本地现象不能直接泛化为正式发布包的问题。具体符号、源码行、复现数据及 Actions 运行链接见[调查记录](./architecture/allocator-retention-and-ci-build-2026-10-29.md)。

## 标准构建

在仓库根目录执行。需要 Bash、Python 3、Rust 的 `aarch64-unknown-linux-musl` 目标、Zig，以及能处理 ELF 的 `llvm-strip` 或 `aarch64-linux-musl-strip`。脚本默认离线构建，首次构建前需准备 Cargo 依赖缓存。

```bash
rustup target add aarch64-unknown-linux-musl
cargo fetch --locked --target aarch64-unknown-linux-musl
bash tools/build-merlin-musl.sh
```

`tools/build-merlin-musl.sh` 会依次完成：

1. C 依赖使用 `tools/zig-cc-aarch64-musl` 和 `tools/zig-ar-aarch64-musl`。
2. 顶层 Rust 二进制使用 `rust-lld`，显式设置 `link-self-contained=yes` 和 `strip=none`。
3. 对未剥离 ELF 执行分配器校验；不符合要求则退出，不能继续作为已验收产物。
4. 保存 `.symbols`，再剥离用于部署的可执行文件。

默认输出：

```text
target/aarch64-unknown-linux-musl/release/sockrocket-cli
target/aarch64-unknown-linux-musl/release/sockrocket-cli.symbols
```

可以设置 `CARGO_TARGET_DIR` 使用独立输出目录，设置 `MERLIN_STRIP_TOOL` 指定剥离工具的可执行文件路径。脚本额外参数传给 Cargo，例如 `--manifest-path path/to/source/Cargo.toml`。功能开关必须由所选源码实际声明；普通工作区不应照抄私有诊断快照的 feature 参数。

这个入口只构建 ARM64 CLI。ARMv7 构建继续使用现有 `cross` 流程；不能把 ARM64 校验器直接用于 ARMv7。

## 分配器验收与来源记录

可手工重跑构建脚本中的检查：

```bash
python3 tools/verify-merlin-allocator.py \
  target/aarch64-unknown-linux-musl/release/sockrocket-cli.symbols
```

校验器读取真实 ELF 符号表，要求 AArch64 架构及已定义的 `malloc`、`free`、`realloc`、`__malloc_context`、`__libc_malloc_impl`，并拒绝包含 `SmpAllocator` 的产物。未定义引用、调试字符串或目标名称都不能代替实现符号。对已剥离文件，校验器会因证据不足而拒绝；应保留与部署文件同一次构建产生的 `.symbols`，不要拿其他版本的符号文件凑数。

每次准备部署时，将以下信息随构建保存在本地来源清单中：

- 源码提交、工作区差异；使用文件覆盖或独立快照时，记录覆盖文件及 SHA256，不能只用 HEAD 代表整个构建。
- `rustc -vV`、`cargo -V`、`zig version`、准确的构建命令、Cargo feature、目标架构和链接选项。
- 实际使用的 `libc.a` 的 SHA256、分配器校验结果，以及剥离版和符号版各自的 SHA256。
- 若来自 Actions，记录提交、运行与任务链接、工具版本及实际容器 digest；不要将本地候选称为 CI 发布产物。

使用 `CARGO_TARGET_DIR` 时，下例路径需相应替换：

```bash
python3 - <<'PY'
from pathlib import Path
import hashlib

base = Path('target/aarch64-unknown-linux-musl/release')
for name in ['sockrocket-cli', 'sockrocket-cli.symbols']:
    path = base / name
    print(hashlib.sha256(path.read_bytes()).hexdigest(), name)
PY
```

来源清单和日志放在本地私有目录，不包含密码、token、完整路由器配置或客户端明细。部署时核对剥离版的哈希与来源清单一致。打包脚本只复制已有二进制，不会修正错误的链接器或分配器。

## 工具链升级时如何复核

升级 Rust、Zig、`cross` 镜像，或改变链接参数后，重新检查未剥离产物、记录哈希，并运行有分配和释放两个阶段的目标设备测试。只测试分配大块后释放不够：本次 Zig 实现可以释放大块，问题机制在多种尺寸的小块 slab。

目标设备测试应使用独立进程与受控内存上限，记录测试前后服务 PID；之后再对真实服务按相同 PID 比较 Rust 存活请求、RSS/匿名页及系统可用内存。进程重启后的下降不能当作自然回收。若分配器内部符号随版本变化，先确认实现和行为，再更新校验器；不能删除校验来让构建变绿。

旧的 `docs/merlin-stability-review-2026-10-15.md` 中 Zig 最终链接方式属于历史构建记录，已由本文替代。调查材料中较早的“musl 分配器”推测也不能替代对具体二进制的核验。
