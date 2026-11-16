# stream-markdown-rs

为 Rust TUI 解析 AI 流式 Markdown 的库。解析器把每次追加转换成带类型、内容变化和生命周期状态的块更新。终端 UI 可以用同一条流逐步显示段落、代码、表格和列表，按块状态决定何时提交最终样式。原生追加延迟、内存占用和更新范围是首要指标。

当前仓库提供回归测试、候选适配器入口和 TypeScript 参考基线。Rust 核心及原生基准将在下一阶段实现。

## 运行验证

```sh
pnpm install --frozen-lockfile
pnpm test
mkdir -p results
pnpm --silent bench > results/reference-node.json
pnpm test:spec --list
```

- `test/behavior` 中的节点行为用例能在参考实现和候选适配器之间切换。
- `test/original` 保存其余原始用例，涵盖依赖旧 token、插件和运行时内部的断言，以及还有待拆出节点断言的混合用例。
- `test:differential` 重放固定种子和真实语料。候选模式逐提交与新建的参考实例对拍。参考模式还记录旧版复用过的 sync 实例与新实例的差异。
- `test:spec` 首次运行下载并校验 CommonMark 0.31.2 和 cmark-gfm 0.29.0.gfm.13 的官方示例。参考源码和真实语料自带，安装后可以离线运行回归测试。

参考源码归档和语料固定在 `markstream-vue` 的 `31668cf54cb74fc46a6b4d0e9e7f0db67fe8bfed`。归档和旧 tokenizer 只服务于验证。原仓库的未提交改动未纳入基线。

## 接入候选实现

```sh
PARSER_CANDIDATE=./path/to/adapter.mjs pnpm test:candidate
PARSER_CANDIDATE=./path/to/adapter.mjs pnpm test:spec
PARSER_CANDIDATE=./path/to/adapter.mjs pnpm --silent bench > results/comparison-node.json
```

Node 验证适配器导出同步的 `createSession({ id, factoryOptions, parseOptions })`，返回 `append(chunk)`、`finish()`、`snapshot()` 和可选的 `dispose()`、`metrics()`。适配器逐次应用候选输出的增量更新，测试需要时才读取当前节点。规范入口调用 `renderSpec(source, { dialect, extensions })`，把同一个核心解析出的节点转换成规范示例的 HTML。

[查看 TUI 接口、块生命周期、缓存和验收设计](docs/design.md)
