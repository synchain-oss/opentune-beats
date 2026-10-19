# OpenTune Beats 协作规范

伴奏的节拍、小节首拍与 tempo map 分析 CLI（Rust）。供 OpenTune MCP Server 的 `beat-grid` skill 调用：skill 下载本仓库 Release 里的二进制与模型，执行 `opentune-beats analyze`，读 stdout 的 JSON。

## 0. 硬规则

1. 提交身份 `DLsnows <noreply@synchain.ca>`，`git commit -s`（DCO）。只暂存显式路径。
2. 零 AI 署名：提交信息、PR 标题与正文、评论、tag、release notes 里不得出现任何 AI 的 `Co-Authored-By`、生成声明或链接。
3. 不写个人路径、个人邮箱。
4. **不得引入任何 GPL / LGPL / AGPL 代码或依赖**（Mixxx、qm-dsp、aubio、Essentia 等只能读论文与文档，不读不抄代码）。新增 crate 前核对许可（MIT / Apache-2.0 / BSD / Zlib / ISC / Unicode 类）。
5. **仓库里不放音频**：基准与测试只提交标注、MIDI 生成参数与脚本；音频在 CI 或本机临时生成。不提交、不在任何文件 / PR / 日志里提及维护者本机的测试歌曲。
6. 模型文件（`*.onnx`）不入库：构建与测试时按锁定的 URL 与 SHA-256 下载。
7. 任何 workflow 不得使用 `pull_request_target`；第三方 action pin 40 位 SHA 并注明版本；workflow 默认 `permissions: contents: read`。
8. 不打 tag、不发布 Release，除非维护者按审批明确要求。

## 1. 分支

- `dev`（默认）、`prod`；主支线 `feature/v1`；子支线 `feat/<slug>`，PR base 为 `feature/v1`。

## 2. 输出契约

- CLI 与 JSON 结构见 `docs/output.md`。`schemaVersion` 只在不兼容改动时递增；字段只增不改名。
- 时间一律秒（`*Sec`），从输入文件开头算；毫秒字段以 `Ms` 结尾。
- 只写 stdout（JSON）与 stderr（日志、进度）；退出码见 `docs/output.md`。

## 3. 语言

- README.md 英文，README.zh-CN.md 为全量中文镜像；docs/ 英文；本文件、PR 描述与评论中文；代码注释英文。

## 4. 本地与 CI

- 本机先跑 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`，再推 `feat/*` 分支；以 CI 为准（macOS universal2 只在 CI 上编）。
- CI：Windows x64（MSVC，静态 CRT `/MT`）与 macOS universal2；`cargo fmt --check`、`cargo clippy -D warnings`、`cargo test`、基准精度检查、许可检查（`cargo deny`）。
