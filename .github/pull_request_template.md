<!--
SPDX-FileCopyrightText: Copyright (c) 2026 Synchain
SPDX-License-Identifier: MIT

PR 模板。标题与正文用中文；正文与提交信息不得含任何 AI 署名行。
规则全文见根目录 CLAUDE.md，CLI 与 JSON 契约见 docs/output.md。
-->

## 这个 PR 做了什么

<!-- 一段话。涉及接口、JSON 字段或告警码时，写明对应的 docs/output.md 章节。 -->

## 为什么

<!-- 动机与取舍。涉及依赖时写明新 crate 的许可与用途；涉及模型时写明来源、版本与 SHA-256。 -->

## 变更类型

- [ ] feat - [ ] fix - [ ] test - [ ] docs - [ ] refactor - [ ] ci - [ ] chore - [ ] harden

## 自查

- [ ] `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test` 已在 CI 上跑过（本机无 Rust 工具链）
- [ ] 改到 CLI 或 JSON：`docs/output.md` 已同步；`schemaVersion` 未在兼容改动中递增，字段只增不改名
- [ ] 失败路径仍只向 stdout 输出一个 JSON 对象，日志与进度在 stderr；退出码符合契约（2 / 3 / 4 / 5）
- [ ] 新增依赖是 MIT / Apache-2.0 / BSD / ISC / Zlib 类许可，已在 PR 里核对；没有 GPL / LGPL / AGPL 代码或依赖
- [ ] 没有提交音频或模型文件；没有引用本机的测试歌曲
- [ ] 没有个人路径、个人邮箱或任何 AI 署名行
- [ ] workflow 改动：第三方 action 仍是 40 位 SHA，默认 `permissions: contents: read`，没有 `pull_request_target`

## 已知限制

<!-- 没有就写「无」。本批（仓库基础设施 + 可编译骨架）已知：analyze 尚未接入引擎，返回 ANALYSIS_FAILED；
version --json 的 beatThis 为 null、models 为空；THIRD-PARTY-NOTICES 目前写明本构建不含第三方 crate，
随依赖加入时改为从 Cargo.lock 生成。 -->
