# 测试与验证记录

本文档汇总 osu-replay-judge 的全部测试：方法、测试集、结果、发现的 bug 与复现方式。

---

## 一、测试方法（三层）

### 1. 存储统计对照（大规模旁证）

以 lazer 数据库 `Score` 表（或 `.osr` 头部）存储的 TotalScore / MaxCombo /
Statistics（300/100/50/miss）为参照，批量比对模拟结果。

- 快，可跑全部语料
- 局限：存储值是**游玩当时版本**的 lazer 代码算出的；lazer 在 2024-2025
  间多次修改判定规则（命中窗口、400ms miss 窗、计分公式重做、slider
  判定），老分数即使引擎完全正确也对不上
- 用途：发现差异线索 → 进入第 2 层定位

### 2. C# 原版逐事件对比（正确性基准）

`osu.Game.Rulesets.Osu.Tests/TestSceneReplayJudgeHarness`（本仓库配套）用
**真实的 DrawableOsuRuleset + OsuScoreProcessor** 无头驱动同一回放，dump
每个判定事件；`difftool.py` 逐事件比对 7 个字段：
**时间 / 物件 / 判定结果 / combo / 分数 / acc / UR**。

运行方式（环境变量控制）：

```
cd D:\Github\osu
OSU_JUDGE_MAP=<path.osu> OSU_JUDGE_REPLAY=<path.osr> \
OSU_JUDGE_OUT=<out.tsv> [OSU_JUDGE_CLASSIC=1] [OSU_JUDGE_MODS=<bits>] \
dotnet test osu.Game.Rulesets.Osu.Tests -c Debug \
  --filter "FullyQualifiedName~TestSceneReplayJudgeHarness.RunReplay"

# Rust 侧：
osu_replay_judge <map.osu> <replay.osr> --timeline t.tsv
python difftool.py t.tsv <out.tsv>
```

- 这是**正确性的唯一基准**：与当前 lazer master 代码逐事件相等
- 慢（15-90s/个，受 4x 实时节流限制），只抽样跑
- harness 自身存在非确定性（异步物件加载与节流时钟竞争，同输入两次
  运行个别事件时刻会浮动 ±1 帧），差 1-4 个纯时刻差视为通过

### 3. 数据库全量批跑（回归）

用 [realm-db-reader](https://github.com/Ohdmire/realm-db-reader) 读取
`client.realm`，导出全部 osu!std 分数的谱面+回放配对（11631 对），批量跑
并按年份分桶统计吻合率，作为修复前后的回归指标。

---

## 二、测试集

| 测试集 | 数量 | 来源 | 覆盖 |
|--------|-----:|------|------|
| 手工收集对（map0-7、rei×2、bloom×2） | 12 | 本机 lazer exports + 文件库 | nomod/DT/HD、lazer+stable 回放、双押谱、密集 tick 谱、长滑条谱 |
| 数据库导出全量 | 11631 | `client.realm`（4086 stable + 7545 lazer，2018-2026） | 全部本机分数 |
| 随机抽样（seed=42） | 10 | 上述全量中随机 | 5 stable + 5 lazer，nomod/DT |
| 重点模式抽样 | 4 | 与存储统计不吻合的近期分数 | 纯 combo 差 / 300↔100 边界两类模式 |
| CL 验证抽样 | 2 | 与存储统计不吻合的 legacy 分数 | classic 模式准确性 |

## 三、结果汇总

### C# 原版逐事件对比（共 26 组）

| 组 | 结果 |
|----|------|
| map0-7（8 组） | **全部 0 差异**（map0 曾 628 差、修复后归零） |
| rei_lazer / rei_stable | 0 差异 |
| bloom_lazer_dt / bloom_stable_dt | 0 差异（lazer+DT、classic+DT） |
| 随机抽样 10 组 | 6 组 0 差异；score4158/5496 各 1-3 个纯时刻差；score6283/7161 修复后各剩 4/1 个纯时刻差 |
| CL 验证 2 组 | score0001 零差异；score0009 仅 2 个 1.3ms 时刻差 |
| CL 追加抽样（seed=2027，5 组） | **全部 0 差异**（含 2 个存储吻合 + 3 个存储不吻合的） |
| CL 追加抽样（seed=2028，6 组） | 5 组 0/纯时刻差；1 组为极端 gimmick 图（连发同毫秒 spinner，见下） |

**37 组中 30 组逐字段全等；其余 6 组合计仅 ~12 个纯判定时刻差
（±1 帧内，计数/combo/分数/acc/UR 全部相同）。

classic（stable 回放）路径累计 19 组：14 组完全零差异、4 组各 1-4 个
纯时刻差、1 组为同毫秒连发 spinner 极端图（见下），无正常图的结果差异。

本轮（seed=2028）还发现并修复了同刻物件更新顺序：lazer 的
`HitObjectContainer` 平局按 `CompareReverseChildID`（后添加者在前）
排序，即**降序**索引更新。**

已知极端边缘：score6590（"Ultra Beginneer"，DT+FL+HR+HD，全谱 dozens
个**完全同毫秒**的连发 spinner）——事件流的中间 combo 序列在连发段
有顺序差，但**最终分数/combo/计数完全一致**（1657309 / 1611 / 全同），
属同毫秒嵌套 tick 与 body 判定交织的最深边缘。**

### 数据库全量（11631 对，按游玩年份）

| 年份 | 数量 | counts+combo 吻合 | +总分吻合 |
|------|-----:|------------------:|----------:|
| 2026 | 1102 | **73%**（修复前 58%） | **57%**（修复前 32%） |
| 2025 | 1219 | 39% | 27% |
| 2024 | 7279 | 14% | 6% |
| ≤2023 | 1938 | ~13% | <1% |

吻合率随游玩时间递减 = lazer 判定规则版本演化（老分数存储值由当年代码
计算）。2026 年剩余 27% 的不吻合主要来自年内较早 build 的规则微调。

### CL（stable 回放）模式专项

classic 路径存储吻合率仅 15-21%，但经 harness 验证：**lazer 自己的
classic 模式重判也与 stable 存储值不同**（ModClassic 是对 stable 的官方
近似：300/100 边界 ±几个、combo ±1-2）。本项目与 lazer 的 classic 模式
逐事件一致——对"游戏"（lazer）准确，对"stable 本身"任何实现都不完全一致。

---

## 四、测试发现并修复的 bug（按发现轮次）

| # | bug | 发现方式 | 影响 |
|---|-----|---------|------|
| 1 | 帧稳定更新节拍：`FrameAccuratePlayback` 默认关闭，补帧不跳过 | map3 首轮对比 | 所有判定时刻 |
| 2 | catch-up 循环：消费回放帧后同帧补一次参考时刻更新 | map3 tail 时刻差 | 补帧时刻 |
| 3 | `TimingPointAt` 无前置红点时回退到**第一个**红点（非默认 1000） | rei 谱首 slider 速度错误 | slider 时长 |
| 4 | 同刻物件按谱面顺序判定 | rei 双押顺序 | combo 序列 |
| 5 | `HandleHit` 强制 miss 先于触发命中计分 | rei 双押 combo | combo/分数 |
| 6 | head 判定守卫缺失（二次按压重复判定） | rei-stable classic | classic 计数 |
| 7 | spinner 外层判定标志未设置（音锁误挡后续物件） | rei-stable | 大面积 miss |
| 8 | `CurvePositionAt` 的 repeat 镜像（奇数 span 折返） | map7 长滑条 tracking | repeat/tail 判定 |
| 9 | 固定 400ms **Miss 窗口**：`ResultFor` 超出 meh 但 ±400ms 内判 Miss | 随机抽样 score6283 | 早/晚点击判定 |
| 10 | slider head 命中（任何结果）触发 `HandleHit` | 随机抽样 score4977 | 强制 miss 链 |
| 11 | 重复控制点隐式分段：首点保留为类型边界、次点丢弃、共享边界点 | score11625（13.9px 路径偏差） | 所有多段贝塞尔 |
| 12 | 输入状态先于按键事件更新（tracking 补判可见新按键） | score11166 repeat | tracking 建立 |
| 13 | 速度的 f32 乘法（`100f × SliderMultiplier` 单精度舍入） | 1 ulp 时长差翻转球位 | 边界时刻 |
| 14 | head `HitAction` 首次悬停按压即记录（`??=`，无论结果） | tracking 按键约束 | tracking 连续性 |

## 五、已知限制

- Relax / Autopilot / Autoplay / Spun Out 回放显式报错（判定机制特殊）
- DT 大图的极个别判定时刻受 harness 侧非确定性影响（±1 帧）
- HP/失败状态、PP 不实现（按需求排除）

## 六、性能

见 [COMPARISON.md](COMPARISON.md)：单回放 0.04-1.2s，比 C# 无头原版快
75-390 倍；全量 11631 对约 19 分钟（8 并发，103ms/个均值）。
