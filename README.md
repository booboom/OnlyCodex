# OnlyCodex

一个基于 [booboom/opencodex](https://github.com/booboom/opencodex) 改造的独立
Tauri 2 桌面应用，让 Codex Desktop 使用任意 OpenAI Chat Completions 兼容模型。

## 架构

- `python/opencodex_proxy/`：指定上游 commit `24379c8` 的原始代理核心，负责
  Responses ↔ Chat Completions 转换、SSE 流、工具调用、推理回放与模型路由。
- `src-tauri/`：桌面生命周期、安全备份/恢复 Codex 配置、启动冻结的 Python sidecar。
- `src/`：Svelte 5 管理界面（仪表盘、供应商、模型、映射、Codex、日志、设置）。

原项目的 CLI/TUI 入口不再作为外部依赖，但其功能已经迁入桌面界面：

| 原功能 | 桌面入口 |
| --- | --- |
| `start` / `stop` / `restart` / `status` | 仪表盘和 Codex 设置 |
| `models` / 启用禁用 / context window | 模型管理 |
| `discover URL` / Provider 测试 | 供应商管理的“测试并同步模型” |
| 映射搜索与自定义目标 | 映射管理，可选择或手输模型 ID |
| 配置备份、恢复、注入 | Codex 设置 |
| 完整模型 catalog 更新 | Start 自动执行，也可手动刷新 |
| 实时代理日志 | 日志页面 |

应用不再内置 OpenCodeX Go 默认上游：所有请求只走你配置的供应商与映射。

应用数据保存在系统应用数据目录。API 密钥不会进入前端 bundle；启动时生成仅供
本机 sidecar 使用的配置。代理强制绑定 `127.0.0.1` 或 `::1`。

## 开发运行

```bash
npm install
npm run tauri dev
```

开发模式会直接使用 `python/` 中的上游核心，需要 Python 3.11+。

## 打包

先构建当前平台的冻结 sidecar，再执行 Tauri 打包：

```bash
PYTHON_BIN=/path/to/python3.11 scripts/build-sidecar.sh
npm run tauri build
```

生成物位于 `src-tauri/target/release/bundle/`。Windows 和 macOS sidecar 必须分别
在对应系统上构建；Tauri 会把平台二进制一起放入 `.exe` / `.app`。

## 安全恢复

启动代理前，应用会原样备份 `~/.codex/config.toml`，随后用 TOML 解析器注入
`opencodex_proxy` provider。停止代理时逐字恢复原配置。备份缺失时应用会拒绝
覆盖现有配置。

上游代码的 MIT 许可证见 [python/UPSTREAM_LICENSE](python/UPSTREAM_LICENSE)。
