# stream-markdown-rs

为 Rust TUI 解析 AI 流式 Markdown 的库。解析器把每次追加转换成带类型、内容变化和生命周期状态的块更新。终端 UI 可以用同一条流逐步显示段落、代码、表格和列表，按块状态决定何时提交最终样式。原生追加延迟、内存占用和更新范围是首要指标。默认 GFM，AI 预判可与 GFM 或 CommonMark 同时开启，运行时插件作为可选宿主层接入。

仓库包含 Rust 批处理和增量解析核心、可选 WASM 插件宿主、规范测试和原生基准。TUI 可以使用 `Parser::append` 获取变化批次，通过 `Parser::finish` 收尾，或用 `parse` 得到完整结构。 将 `Options::math` 与 `Options::footnotes` 设为 `true` 可启用数学和命名脚注扩展。AI 预览可为打开的段落和标题交付临时行内覆盖，关闭节点时交付最终结构。

## 运行验证

使用 Rust 1.88 或更新工具链及 pnpm。

```sh
cargo test --locked --workspace
pnpm install --frozen-lockfile
pnpm test
pnpm test:spec --native
pnpm test:spec --native-stream
mkdir -p results
pnpm --silent bench > results/reference-node.json
pnpm test:spec --list
```

- `test/behavior` 中的节点行为用例能在参考实现和候选适配器之间切换。
- `test/original` 保存其余原始用例，涵盖依赖旧 token、插件和运行时内部的断言，以及还有待拆出节点断言的混合用例。
- `test:differential` 重放固定种子和真实语料。候选模式逐提交与新建的参考实例对拍。参考模式还记录旧版复用过的 sync 实例与新实例的差异。
- `test:spec` 首次运行下载并校验 CommonMark 0.31.2 和 cmark-gfm 0.29.0.gfm.13 的官方示例。`--native` 检查完整 Rust 解析，`--native-stream` 检查逐 Unicode 字符追加并应用变化后的结果。参考源码和真实语料自带，安装后可以离线运行回归测试。

参考源码归档和语料固定在 `markstream-vue` 的 `31668cf54cb74fc46a6b4d0e9e7f0db67fe8bfed`。归档和旧 tokenizer 只服务于验证。原仓库的未提交改动未纳入基线。

## 接入候选实现

用本仓库的 Rust 候选运行兼容性测试：

```sh
cargo build --locked --release --example node_candidate
node --test test/native-candidate.test.mjs
PARSER_CANDIDATE=./scripts/native-candidate.mjs pnpm test:candidate
```

Node 验证桥在同一个 Rust 会话上逐段追加，并把 Rust 变化应用到旧版节点视图。此桥用于兼容性测试，Rust TUI 和原生性能基准直接调用 Rust API。

```sh
PARSER_CANDIDATE=./path/to/adapter.mjs pnpm test:candidate
PARSER_CANDIDATE=./path/to/adapter.mjs pnpm test:spec
PARSER_CANDIDATE=./path/to/adapter.mjs pnpm --silent bench > results/comparison-node.json
```

Node 验证适配器导出同步的 `createSession({ id, factoryOptions, parseOptions })`，返回 `append(chunk)`、`finish()`、`snapshot()` 和可选的 `dispose()`、`metrics()`。适配器逐次应用候选输出的增量更新，测试需要时才读取当前节点。规范入口调用 `renderSpec(source, { dialect, extensions })`，把同一个核心解析出的节点转换成规范示例的 HTML。

[查看流式预判、Rust 接口、插件、块缓存与验收设计](docs/design.md)

[原生性能基准](crates/bench/README.md) · [WASM 插件宿主与协议](crates/plugin-host/README.md)
