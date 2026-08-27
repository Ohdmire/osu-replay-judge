# osu-replay-judge

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](Cargo.toml)
[![Validation](https://img.shields.io/badge/harness%20validation-37%20cases%20%7C%2030%20exact--zero-brightgreen.svg)](TESTING.md)

用 Rust 独立实现的 osu!standard 模式回放判定模拟器，以 [osu!lazer](https://github.com/ppy/osu)（2025-08 master）为参照，逐事件复刻其判定语义。

给定一张 `.osu` 谱面和一个 `.osr` 回放，输出**每一时刻**的判定事件流：
分数、combo、acc、unstable rate、HP 血量 —— 与游戏内观看该回放时的
ScoreProcessor / HealthProcessor 状态一致。无任何 C# 运行时依赖，单回放
0.04~1.2s（比无头驱动 C# 原版快 75~390 倍）。

## 用法

```
osu_replay_judge <beatmap.osu> <replay.osr> [--mode classic|lazer] [--timeline <out.tsv>] [--json]
```

- `--mode`：默认按回放版本自动选择 —— stable 回放（version < 30000000，
  即 `ModClassic` 路径：LegacyHitPolicy 音锁 + 经典 slider 判定），lazer
  回放走默认规则（StartTimeOrderedHitPolicy + 乱序强 miss）。
- `--timeline`：输出每个判定事件的 TSV（时间、物件、结果、combo、分数、
  acc、UR、时间偏移）。
- `--json`：以 JSON 输出摘要（与 `.osr` 头部记录对照）。

同时输出两个总分：

- **standardised**：lazer 原生分（含 mod 倍率：DT 1.23x、HR 1.09x、HD 1.04x 等）
- **classic view**：lazer 经典显示分（`ScoreInfoExtensions` 的换算公式）

```
$ osu_replay_judge map.osu replay.osr --timeline out.tsv
=== osu! replay judgement (classic mode) ===
player: ATRI1024
mods:   DT (rate 1.5x, score multiplier 0.9212)
total score (standardised): 1167967
max combo: 1026
accuracy:  98.763291%
unstable rate: 95.1326
counts: 300=722 100=21 50=0 miss=0
```

## 安装

```
cargo install --git https://github.com/Ohdmire/osu-replay-judge
# 或本仓库克隆后
cargo build --release
```

依赖仅 [lzma-rs](https://crates.io/crates/lzma-rs)（.osr 的 LZMA 解压，纯 Rust）。

## 支持范围

- **谱面**：osu! file format v4 ~ v14 与 lazer 格式（v128+）
- **回放**：stable（.osr legacy，含 DT/NC/HT/HR/EZ/HD/FL/SD/PF/NF）与
  lazer 原生（version ≥ 30000000）；自动按版本选择 classic / lazer 判定路径
- **不支持**（显式报错）：Relax / Autopilot / Autoplay / Spun Out
  （判定机制特殊）；HP / 失败状态与 PP 不实现

## 实现语义（与 lazer 的一致性）

以下全部按源码逐行移植（`osu.Game` / `osu.Game.Rulesets.Osu` /
osu-framework），包括浮点类型与运算顺序（f32 向量、f64 时间、单精度舍入点）：

- **谱面**：`.osu` 解析、难度钳制、AR 缺省、时序点（红/绿点、SV、tick
  生成开关）、物件排序、堆叠算法（新旧两版）
- **曲线**：`SliderPath` + `PathApproximator`（线性/完美圆/贝塞尔/B-spline/
  catmull 及 6px catmull 优化）；重复控制点的隐式分段（边界点共享、曲线
  连续）；期望距离的截断/延伸
- **物件**：slider 速度/tick 距离（含 stable 的浮点兼容写法）、嵌套物件
  生成（头/tick/repeat/tail，10ms 末端 tick 抑制）、spinner 所需圈数与
  bonus tick
- **判定窗口**：OD 线性映射、`floor(x) - 0.5`、固定 400ms Miss 窗口
  （超出 meh 但 ±400ms 内的点击判 Miss）
- **更新模型**：完整复刻 `FrameStabilityContainer` + `FramedReplayInputHandler`
  的 60fps 帧稳定回放：参考时钟按 1000/60×rate 累积、每游戏帧至多消费一
  个回放帧（在其精确时刻更新）、帧间在 16.67ms 网格补帧、光标帧间线性
  插值、DT 的双迭代网格、`CurvePositionAt` 的 repeat 镜像
- **输入**：按键按下→圆心半径内 receptor 消费、事件传播顺序、同刻物件
  降序更新；`LegacyHitPolicy`（stable 音锁、堆叠锁、滑头遮挡）与
  `StartTimeOrderedHitPolicy`（乱序命中强 miss，且强 miss 先于触发命中计分）
- **Slider**：`SliderInputManager` 的 tracking 状态机（follow 圈、2.4x 扩
  展半径、头按键约束 `timeToAcceptAnyKeyAfter`、`HitAction` 首按锚定）、
  晚击头部的补判（`PostProcessHeadJudgement`）、tick/repeat/tail 判定
  （tail -36ms 宽容、顺序门）、classic 比例判定与 lazer 忽略判定
- **Spinner**：旋转角累积（±180° 归一化、速率缩放）、`SpinnerSpinHistory`
  防倒转计圈、bonus tick（Small/Large）、进度阈值 300/100/50
- **计分**：`ScoreProcessor` 全状态机（combo、acc、bonus/combo 部分进度、
  自动演奏最大值模拟）、银行家舍入、V2 mod 倍率、UR（Welford 在线算法，
  按游玩速率折算）
- **血量**：两条处理器路径按模式分叉（`health.rs`），与 lazer 的
  处理器选择一致——
  - **lazer 模式**：`HealthProcessor` + `DrainingHealthProcessor` +
    `OsuHealthProcessor`——DR 相关扣血表、combo 尾 Perfect/Good
    +0.07/0.05/0.03 加成（`ComboResult` 逐判定降级）、beat break 段无
    drain（`PeriodTracker` 语义：break 扩到前后物件间隙）、满血模拟解
    drain rate（最低血 0.99/0.9/0.4@HP0/5/10 目标二分逼近）；
  - **classic 模式**（stable 回放 => `ModClassic` "Classic health" 默认
    开）：`OsuLegacyHealthProcessor` → `LegacyDrainingHealthProcessor`
    ——stable 风格迭代 `testDrop` solver（0.00025 起步、四个 fail 条件
    ×0.96/×0.94 自适应、`HpMultiplierNormal` ×1.01 收敛），实况扣血
    乘该系数且**无 combo 尾加成**（`CreateHealthProcessor` 换处理器）；
  - 两者都输出逐判定 HP 曲线 + drain 窗口供下游任意时刻重建。

## 模块结构

```
src/
├── main.rs      CLI
├── beatmap.rs   .osu 解析（控制点/物件/曲线字符串/隐式分段）
├── process.rs   难度 mod、物件默认值、堆叠、嵌套生成
├── path.rs      SliderPath + PathApproximator（f32 逐位移植）
├── replay.rs    .osr 二进制 + LZMA 解压、帧修正
├── engine.rs    判定引擎（帧稳定主循环、hit policy、tracking、spinner）
├── score.rs     ScoreProcessor（分数/combo/acc/UR）+ classic 显示分
├── health.rs    HealthProcessor（扣血表/combo尾加成/break/drain rate 求解）
└── mods.rs      legacy mod 位、V2 倍率、速率
```

## 验证

以真实 `DrawableOsuRuleset` 无头驱动的 C# 基准（位于本地 lazer 仓库的
`TestSceneReplayJudgeHarness`）逐事件比对 7 个字段：

- **37 组真实回放**（nomod/DT/HD/HR/FL、lazer+stable、2018-2026）：
  **30 组逐字段全等**，6 组各 1-4 个 ±1 帧时刻差，1 组同毫秒连发
  spinner 极端图（最终分/combo/计数仍全等）
- **11631 对数据库导出真实配对**全量批跑（103ms/个）：2026 年分数与
  存储统计吻合率 73%（+总分 57%），吻合率随游玩年份递减 = lazer 判定
  规则本身的版本演化
- 测试过程累计发现并修复 **15 个**判定语义差异

详见 [TESTING.md](TESTING.md)（方法/测试集/结果/bug 日志/复现命令）与
[COMPARISON.md](COMPARISON.md)（一致性详表 + 性能对比）。

## 致谢

- 判定语义参照 [ppy/osu](https://github.com/ppy/osu)（MIT）
- 曲线数学参照 [ppy/osu-framework](https://github.com/ppy/osu-framework)
  的 `PathApproximator`
- 测试数据导出使用 [Ohdmire/realm-db-reader](https://github.com/Ohdmire/realm-db-reader)
  读取 lazer 的 realm 数据库

## License

[MIT](LICENSE) © 2026 ATRI1024
