# osu-replay-judge

用 Rust 独立实现的 osu!standard 模式回放判定模拟器，以 osu!lazer（本仓库
`D:\Github\osu`，2025-08 master）为参照，逐事件复刻其判定语义。

给定一张 `.osu` 谱面和一个 `.osr` 回放，输出**每一时刻**的判定事件流：
分数、combo、acc、unstable rate —— 与游戏内观看该回放时的 ScoreProcessor
状态完全一致。

## 用法

```
osu_replay_judge <beatmap.osu> <replay.osr> [--mode classic|lazer] [--timeline <out.tsv>] [--json]
```

- `--mode`：默认按回放版本自动选择 —— stable 回放（version < 30000000，
  即 `ModClassic` 路径：LegacyHitPolicy 音锁 + 经典 slider 判定），lazer
  回放走默认规则（StartTimeOrderedHitPolicy）。
- `--timeline`：输出每个判定事件的 TSV（时间、物件、结果、combo、分数、
  acc、UR、时间偏移）。
- `--json`：以 JSON 输出摘要（与 `.osr` 头部记录对照）。

同时输出两个总分：
- **standardised**：lazer 原生分（含 mod 倍率）。
- **classic view**：lazer 经典显示分（`ScoreInfoExtensions` 的换算公式）。

## 判定语义（与 lazer 的一致性）

以下全部按源码逐行移植（`osu.Game` / `osu.Game.Rulesets.Osu` /
osu-framework）：

- **谱面**：`.osu` 解析（v4-v14、LAZER 格式）、难度钳制、AR 缺省、
  时序点（红/绿点、SV、tick 生成开关）、物件排序、堆叠算法（新旧两版）。
- **曲线**：`SliderPath` + `PathApproximator`（线性/完美圆/贝塞尔/B-spline/
  catmull 及 6px catmull 优化），f32 精度与 lazer 相同；期望距离的
  截断/延伸。
- **物件**：slider 速度/tick 距离（含稳定的浮点兼容写法）、嵌套物件生成
  （头/tick/repeat/tail，含 10ms 末端 tick 抑制）、spinner 所需圈数与
  bonus tick。
- **判定窗口**：OD 线性映射、`floor(x) - 0.5`、固定 400ms miss 窗口。
- **更新模型**：完整复刻 `FrameStabilityContainer` + `FramedReplayInputHandler`
  的 60fps 帧稳定回放：参考时钟按 1000/60×rate 累积、每游戏帧至多消费一
  个回放帧（在其精确时刻更新）、帧间在 16.67ms 网格补帧、光标在帧间线性
  插值、`CurvePositionAt` 的 repeat 镜像。
- **输入**：按键按下→圆形 receptor（圆心半径内）消费；事件传播顺序；
  `LegacyHitPolicy`（stable 音锁、堆叠锁、滑头遮挡）与
  `StartTimeOrderedHitPolicy`（乱序强 miss，且强 miss 先于触发命中计分）。
- **Slider**：`SliderInputManager` 的 tracking（follow 圈、2.4x 扩展半径、
  头按键约束 `timeToAcceptAnyKeyAfter`）、晚击头部的补判
  （`PostProcessHeadJudgement`）、tick/repeat/tail 判定（tail -36ms 宽容、
  顺序门）、classic 比例判定与 lazer 忽略判定。
- **Spinner**：旋转角累积（±180° 归一化、rate 缩放）、
  `SpinnerSpinHistory` 防倒转计数、bonus tick、进度阈值 300/100/50。
- **计分**：`ScoreProcessor` 全状态机（combo、acc、bonus/combo 部分进度、
  自动演奏最大值模拟）、银行家舍入、V2 mod 倍率、UR（Welford 在线算法，
  按游玩速率折算）。

## 与 lazer 实机的逐事件对照验证

基准：在 `osu.Game.Rulesets.Osu.Tests` 中内置了 `TestSceneReplayJudgeHarness`
（环境变量 `OSU_JUDGE_MAP/OSU_JUDGE_REPLAY/OSU_JUDGE_OUT/OSU_JUDGE_MODS/
OSU_JUDGE_CLASSIC`），用**真实的 DrawableOsuRuleset** 无头驱动回放并 dump
每个判定事件。`difftool.py` 逐事件比对时间/物件/结果/combo/分数/acc/UR。

10 组真实回放（8 lazer + 1 stable + 1 lazer 双打谱）的最终结果：

| 测试 | 模式 | 结果 |
| --- | --- | --- |
| map1/3/4/5/6/7、rei-lazer | lazer | **0 差异**（每个事件逐字段相等） |
| rei-stable | classic | **0 差异**（且与 .osr 头部的 300/100/50/miss/combo 完全吻合） |
| map2 (DT) | lazer | 1301 事件中仅 2 个 slider body 判定时刻差 ~12ms（结果/分数/UR 全同） |
| map0 (DT) | lazer | 一处 tail tracking 边缘翻转（DT 更新网格相位）后级联 |

即：nomod 与 classic 回放已做到逐事件全等；DT 回放在极个别
tracking/tail 边缘上仍受更新网格相位影响（±1 帧），其余全部一致。

## 限制

- 不支持 Relax / Autopilot / Autoplay / Spun Out 回放（会显式报错）。
- HP/失败状态不模拟（不影响分数）。
- PP 不实现（按需求排除）。

## 依赖

- Rust（`cargo build --release`）
- `lzma-rs`（.osr LZMA 解压，纯 Rust）

## 对比文档

验证方法、12 组真实回放的逐事件一致性结果、与 C# 原版的耗时对比
（75~390 倍），见 [COMPARISON.md](COMPARISON.md)。

## 数据库导出的真实测试集

用 [realm-db-reader](https://github.com/Ohdmire/realm-db-reader) 读取
lazer 的 `client.realm`，导出 221 对数据库配对的真实谱面+回放到
`testdata/dbtests/`（含 `manifest.json`：数据库权威的分数/combo/统计）。
导出器位于该仓库的 `examples/export_tests.rs` 与 `dump_scores.rs`。
