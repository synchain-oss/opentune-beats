# OpenTune Beats

一个小巧的原生命令行工具：分析伴奏的节拍、小节首拍和速度变化（tempo map）。

它是 [OpenTune MCP Server](https://github.com/synchain-oss/opentune-mcp-server) 里 `beat-grid`
skill 的节拍网格引擎。AI 助手用它按段落测出伴奏的速度，再据此在 OpenTune 里修正人声的节奏。

> **状态：** 开发中，尚未发布。

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

用已知 tempo map（含变速）的 MIDI 渲染出的伴奏来测。目标：拍点绝对误差的中位数小于 5 ms，
95 分位小于 15 ms；另报 ±70 ms 容差下的 F 值。仓库里只放标注与生成脚本，不放任何音频。

## 平台

CI 构建 Windows x64 与 macOS（universal2）两种二进制。

## 许可

MIT，见 [LICENSE](LICENSE)。Beat This! 的代码与模型权重都是 MIT 许可。模型的部分训练数据受版权保护
或采用受限的知识共享许可，见 [NOTICE](NOTICE)。

OpenTune Beats 由 Synchain 制作，不是 OpenTune 官方项目。
