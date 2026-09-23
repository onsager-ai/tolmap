# tolmap terminology / 术语（所有者 2026-09-23 定稿，第二版）

Rule: **things that exist in code keep their code names; structure that tolmap discovers gets a map name.** A metaphor is used only where the code has no word for the thing, and only where the map word carries information (bounded area, traffic, hidden passage, border). Code objects are never renamed into metaphors: names are what people search and jump by.

规则：**代码里本来有名字的东西用软件原名；tolmap 算出来、代码里没有名字的结构才用地图词。** 隐喻只用在代码没有对应概念、并且地图词本身带信息（有边界的一片、流量、看不见的通道、边境）的地方。代码对象不改成隐喻名：名字是人搜索和跳转的依据。

Only UI text and docs change in this round; code and schema field names stay as they are (renaming `districts`, `P` etc. would ripple through the acceptance fixtures and belongs in a separate migration).

## 1. Code objects — native names / 代码对象：用原名

| 中文 | English | Notes / schema today |
|---|---|---|
| 组织 | Organization | future multi-repo view |
| 仓库 | Repository (repo) | map document |
| 包 / 应用 | Package / App | a source root in a monorepo, e.g. dify `api/`, `web/` |
| 目录 | Directory | |
| 文件 | File | `files`, `nodes` |
| 类 | Class | symbol kind `class` |
| 函数 | Function | symbol kind `func` |
| 方法 | Method | symbol kind `method` |
| 内部函数 | Nested function | prototype kind 3 |
| 模块级代码 | Module-level code | imports, constants, top-level statements; prototype `SR` |
| 依赖（import） | Import | `edges` |
| 引用 | Reference | symbol → symbol; today `uses` is file-level |
| 代码行 | Code lines | lines excluding blank, comment-only and docstring lines |
| 文件占地 | File footprint | a file's area on the map, ∝ code lines; comparable only within one district; `P` (parcels) |

## 2. Discovered structure — map names / 算出来的结构：用地图词

| 中文 | English | Meaning | Old names |
|---|---|---|---|
| 区 | District | first-level community of files | 区块 |
| 街区 | Neighborhood | second-level community inside a district | 瓣 lobe |
| 道路 | Road | an aggregated dependency drawn between regions (generic) | — |
| 主干道 | Avenue | road between two districts | 区块道路 |
| 街道 | Street | road between neighborhoods, or neighborhood ↔ district | 瓣道路 |
| 交通枢纽 | Hub | role of a file imported by ≥ 30 files (the product's `hub` landmark) | 枢纽 |
| 地标 | Landmark | existing product term: entry, bridge, hub, capital, hazard | — |
| 地下通道 | Tunnel | two files often changed together with no import between them | 隐藏耦合 hidden coupling |
| 边境文件 | Border file | role of a file whose district membership is uncertain | 边界文件 boundary file |
| 主岛 / 离岛 | Mainland / Island | existing product terms: connected body / unconnected districts | — |
| 航线 | Route | aggregated dependency between repositories (future) | — |
| 世界视图 | World view | a view of several repos at once; a view name, not a noun for repos (future) | 星球 / 大陆（弃用） |

## 3. Internal geometry and method terms (docs only, not UI) / 内部用语

| 中文 | English | Meaning |
|---|---|---|
| 轮廓 | Outline | boundary polygon of a district or neighborhood |
| 缝隙 | Gutter | white gap between neighborhoods |
| 站点 | Site | power-diagram generator; never anchor lines or labels here |
| 质心 | Centroid | centre of the displayed file footprint; all lines, labels and hit areas anchor here |
| 细节层级 | Level of detail (LOD) | district names → neighborhood names → file internals → class members, gated by on-screen size |
| 共识分组 | Consensus partition | co-association of many seeded Leiden runs, re-clustered (experiment) |
| 归属把握 | Membership confidence | share of runs in which a file stays with its district |

## 4. Withdrawn / 已撤回

大陆 Continent（仓库）、大区 Region（包）、建筑 Building（文件）、单元 Unit（类）、房间 Room（函数/方法）、隔间 Alcove（内部函数）、大堂 Lobby（模块级代码）、小路 Path（文件 import）、星球 Planet（多仓库）。理由：这些东西在代码里本来有名字，隐喻名需要括号解释、不能检索，还和软件词撞车（unit test、cloud region）。
