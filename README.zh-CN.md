# OpenTune Beats

一个小巧的原生命令行工具：分析伴奏的节拍、小节首拍和速度变化（tempo map）。

它是 [OpenTune MCP Server](https://github.com/synchain-oss/opentune-mcp-server) 里 `beat-grid`
skill 的节拍网格引擎。AI 助手用它按段落测出伴奏的速度，再据此在 OpenTune 里修正人声的节奏。

> **状态：** 早期版本（0.1.0）。二进制和模型文件在 [Releases](https://github.com/synchain-oss/opentune-beats/releases) 页面。

[English](README.md)

## 功能

- **输入：** 伴奏或原曲混音的 WAV 文件。不要用分离出来的人声：节拍跟踪在纯人声上不可靠。
  只有干声时，请自己提供速度（BPM、click 音轨或 MIDI 文件）。
- **输出：** 打印到 stdout 的 JSON：
  - 每一拍和每个小节首拍的时间，带置信度；
  - tempo map：恒速段的起止、BPM、拍号候选与置信度，并统一半速 / 倍速的读法；
  - 对照音频的起音包络，把拍点精修到毫秒级；
  - 工具版本、模型名与 SHA-256、处理参数。
- **模型：** `--model final`（默认，约 80 MB，最准）或 `--model small`（约 10 MB）。

OpenTune 工程只有一个速度。分段的 tempo map 用来修正人声节奏；要不要设工程速度、用哪一段，
由用户另外决定。

## 原理

节拍与小节首拍的激活值来自 [Beat This!](https://github.com/CPJKU/beat_this)（CPJKU，ISMIR 2024），
经纯 Rust 移植 [beat-this-rs](https://github.com/danigb/beat-this-rs) 运行：不需要 Python、PyTorch
或任何系统库。恒速段切分、半速 / 倍速统一和毫秒级精修是我们自己的代码，按公开的论文与描述编写。

## 精度

用已知 tempo map 的 MIDI 渲染或按参数生成的伴奏来测，覆盖变速、无拍空白与不在拍上的人声式起音。
目标：拍点绝对误差的中位数小于 5 ms，95 分位小于 15 ms；另报 ±70 ms 容差下的 F 值。同一批合成素材
还给 tempo map 本身设门槛：稳定素材必须只有 1 段、变速素材的分段数与真值相差不超过 1、每段 BPM 与
真值相差不超过 1%、稳定素材置信度不低于 0.8，且精修不得让拍间隔离散度变大。多段变速素材的拍点 F 值
照报但不设门槛，因为模型在变速后会沿用旧速度一段时间（见 [docs/output.md](docs/output.md) 的局限）。
仓库里只放标注与生成脚本，不放任何音频。

## 局限

- 变速会被滞后跟随：模型在变速后会沿用旧速度一段时间，变速点附近的拍点可能晚一帧甚至更多。
- 空白不会被补出来：`beats` 只保留模型报出的拍点。两段之间的空白由 `segments[].gapAfterSec` 描述，
  需要空白内拍点的调用方按 `bpm` 推导。
- 精修只在明确更好时才采用；否则该段保留模型的原始拍点并标 `refinement: "skipped"`。
- 置信度是拟合质量，不是「速度正确的概率」。
- 只承诺 WAV；输入上限 2 GiB、一小时，采样率须在 8 000 至 384 000 Hz；RF64/BW64 直接给出清楚的错误。

## 平台

CI 构建 Windows x64 与 macOS（universal2）两种二进制。发布资产——两个压缩包、三个模型文件与
`SHA256SUMS`——见 [Releases](https://github.com/synchain-oss/opentune-beats/releases) 页面；
发版流程见 [docs/RELEASE.md](docs/RELEASE.md)。

## 许可

MIT，见 [LICENSE](LICENSE)。Beat This! 的代码与模型权重都是 MIT 许可。模型的部分训练数据受版权保护
或采用受限的知识共享许可，见 [NOTICE](NOTICE)。

OpenTune Beats 由 Synchain 制作，不是 OpenTune 官方项目。
