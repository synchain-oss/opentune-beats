# 参与 OpenTune Beats

感谢你的关注。OpenTune Beats (by Synchain) 是一个 MIT 许可的 Rust 命令行工具：输入伴奏或整曲混音的
WAV，输出节拍、小节首拍与 tempo map 的 JSON，供 OpenTune MCP Server 的 `beat-grid` skill 调用。
项目处于早期，代码正在陆续加入；文中提到的脚本与 CI 检查随首批代码与 workflow 加入。issue 与 PR 都
欢迎，开工之前请把本文读完——尤其是 **§7 不会被接受的改动**。较大的改动请先开 issue 讨论。

## 0. 语言政策

- issue / PR 可以用**中文或英文**；维护者用你使用的语言回复。维护者内部沟通用中文。
- `README.md` 是英文，`README.zh-CN.md` 是它的**全量中文镜像**，两者的标题结构（层级与顺序）必须一致。
  改其中一份，必须在同一个 PR 里同步另一份。
- `docs/` 用英文；本文件与 `CLAUDE.md` 用中文。
- 代码注释、错误信息、日志用英文。

## 1. 行为准则

参与本项目即视为同意：讨论对事不对人，不人身攻击，不泄露他人的隐私信息；安全问题按
[SECURITY.md](./SECURITY.md) 私下报告。举报渠道：**contact@synchain.ca**。

## 2. 开发者原创声明（DCO）与署名

每个 commit 必须带 `Signed-off-by:` 尾行：

```
git commit -s
```

本项目采用 DCO，**不采用 CLA**。这条由 `branch-gate` 机器强制（PR → `dev` / `prod`；合并提交豁免）。
忘签的补救：

```
git commit -s --amend     # 最后一个 commit
git rebase --signoff      # 一段区间
```

提交信息与 PR 标题、正文里**不要带 AI 工具的署名行**（例如把 AI 助手列为合著者的尾行、会话链接尾行、
「由某某 AI 生成」一类的标语）。你可以借助 AI 工具写代码，但提交由你签署、由你负责；CI 会检查署名。

## 3. 分支模型

- 默认分支是 `dev`；`prod` 是发布分支，版本 tag 打在 `prod` 上。两者都只由维护者在审核后合入。
- **内部贡献者**：主支线是 `feature/v1`；从它开 `feat/<slug>` 子支线，PR 回 `feature/v1`。
  一张卡一条子支线一个 PR。主支线经维护者预览并明确批准后，才合入 `dev`。
- same-repo 的 PR：到 `dev` 只接受来自 `feat/*` / `feature/*`（外加 `dependabot/*`）的分支；到 `prod`
  只接受 `dev`。
- **外部贡献者**：fork 本仓 → 用**任意分支名**（请不要用 `dev` / `prod` / `feature/v1`）→ PR 到 `dev`。
  - fork PR 只跑**无 secrets** 的检查，且需维护者批准 workflow run 后才开始跑。
  - AI review 不会对 fork PR 自动运行。
  - 本项目**任何 workflow 都不使用 `pull_request_target`**，这是不接受个案豁免的安全禁令。

## 4. Commit 规范

`type(scope): 描述`，描述可中可英。type 取值：`fix` / `feat` / `docs` / `chore` / `refactor` / `test` /
`ci` / `style` / `perf` / `revert` / `harden`。

## 5. 环境搭建

- Rust：版本固定在 `rust-toolchain.toml`，`rustup` 会自动装它。需要 `rustfmt` 与 `clippy` 两个组件。
- 构建与测试：`cargo build`、`cargo test`。Windows 上静态链接 CRT（`.cargo/config.toml` 里的
  `+crt-static`），所以产物不依赖 VC++ 运行库。
- 模型文件**不入库**：需要分析时，把 `mel_spectrogram.onnx` 与 `beat_this_final.onnx`（或
  `beat_this_small.onnx`）放进 `models/`（已被 `.gitignore` 忽略）。
  `node scripts/fetch-models.mjs --dir models --model final` 会按锁定的 URL 下载并逐个校验
  SHA-256；模型的来源见 [NOTICE](./NOTICE)，期望的 digest 由二进制发布
  （`version --json`，见 [docs/output.md](./docs/output.md)），也定义在 `src/models.rs`。`cargo test` 不需要模型，
  只有真实运行 `analyze` 与下面的精度检查需要。
- 本机先跑 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`；开 PR（或对 `feat/*` 分支手动触发 `workflow_dispatch`）后以 CI 为准，因为推 `feat/*` 本身不触发 CI；macOS universal2 只在 CI 上编，精度基准（`accuracy` job）在 ubuntu 上跑。

## 6. 提 PR 前的本地检查

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
node scripts/ci-checks.mjs pins      # 署名检查器与其模式表未被改动
# 身份规则与 workflow 扫描的固定用例；模型下载脚本与合成器的回归用例
node --test scripts/ci-checks.test.mjs scripts/fetch-models.test.mjs bench/gen.test.mjs
node scripts/gen-notices.mjs --check     # THIRD-PARTY-NOTICES 与发布依赖图一致
node scripts/ci-checks.mjs files     # 无 AI 署名
node scripts/ci-checks.mjs privacy   # 无个人路径与个人邮箱
node scripts/ci-checks.mjs assets    # 无音频与模型文件
node scripts/ci-checks.mjs pull-request-target
```

依赖许可由 `cargo deny` 检查（`deny.toml`；CI 的 `licenses` job）。CI 的 `beats-gate` 是唯一必需的检查。

`node scripts/ci-checks.mjs` 的文本检查扫描**已跟踪与未被忽略的文件**；被 `.gitignore` 忽略的（如本地
`models/`、`target/`、`.cache/`）不扫描。

合成伴奏的毫秒级精度检查由 CI 的 `accuracy` job 跑：`bench/gen.mjs` 按 `bench/tempo-map.json`
与 `bench/maps/*.json` 逐张渲染音频（不入库），`bench/score.mjs` 比较 `analyze` 的输出。拍点门槛
（中位误差 < 5 ms、P95 < 15 ms、±70 ms F ≥ 0.95）之外的 tempo map 门槛由每张 map 的
`acceptance` 声明：分段数与真值相符（稳定素材恰好 1 段，变速素材 ±1 段）、每段 BPM 误差 ≤ 1%、
稳定素材置信度 ≥ 0.8、精修后拍间隔离散度不明显大于原始。本机有模型与二进制时可以照做：

```bash
node bench/gen.mjs --map bench/maps/gaps.json --out bench/out/gaps.wav
./target/release/opentune-beats analyze bench/out/gaps.wav --models-dir models > bench/out/gaps.json
node bench/score.mjs --map bench/maps/gaps.json --analysis bench/out/gaps.json
```

## 7. ★ 不会被接受的改动

- **任何 GPL / LGPL / AGPL 代码或依赖**，包括从 Mixxx、qm-dsp、aubio、Essentia 等项目复制进来的实现。
  参考论文与文字描述可以，读代码再照着重写不行。
- **往仓库里放音频或模型**：基准与测试只提交标注、生成参数与脚本；音频在 CI 或本机临时生成。
  也不要在任何文件、PR 或日志里提及维护者本机的测试歌曲。
- **不兼容的输出契约改动**：删除或改名 JSON 字段、改字段语义或单位、在兼容改动中递增 `schemaVersion`。
  告警码与错误码只能追加。契约见 [docs/output.md](./docs/output.md)。
- **往 stdout 写 JSON 以外的内容**：日志与进度一律走 stderr；失败时 stdout 仍要有一个错误 JSON。
- **个人路径、个人邮箱或任何 AI 署名行**出现在文件、注释、测试快照或 PR 文本里。
- **不锁定的 CI 供应链**：第三方 action 不用 40 位 SHA、使用 `pull_request_target`、workflow 默认权限
  宽于 `contents: read`、二进制工具不锁版本与 sha256。
- **降低安全相关测试的覆盖**，或把输入导致的 panic 当作已知问题放过。

## 8. 评审流程与期望响应时间

- 维护者一般在 3 个工作日内首次响应；小改动更快，大改动请先开 issue 对齐。
- 每个 same-repo PR 都会跑自动化 review。**处理完所有 comment，不止 bot 的**：行内评论、review 正文与
  总结里的条目都算，逐条回复、修改或说明不改的理由，然后 Resolve。bot 的意见不必照单全收，但必须回应。
- 未解决的对话会阻止合并。PR 的合并由维护者执行，不使用 auto-merge。
- 安全问题**不要**开公开 issue，走 [SECURITY.md](./SECURITY.md)。

## 9. 发布流程（仅维护者）

- 版本号真源是 `Cargo.toml` 的 `version`，遵循 SemVer。
- `dev` → `prod` 经维护者审核批准；在 `prod` 上推 `vX.Y.Z` tag 触发发版（流程见
  [docs/RELEASE.md](./docs/RELEASE.md)，`workflow_dispatch` 演练不建 Release）。
- 发布资产：`opentune-beats-<ver>-win-x64.zip`、`opentune-beats-<ver>-macos-universal2.tar.gz`、
  三个模型文件与 `SHA256SUMS`。
