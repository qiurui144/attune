# UI 视觉统一 spec — 用户校正(2026-05-25 14:50)

> 校正主 spec `2026-05-25-ui-visual-unification-spec.md`(agent a0cbb49 起草中)的视觉方向。
> **借鉴 spacemit:布局 + 层级结构** / **不借鉴 spacemit:色彩(过暗)**

## 1. 用户原话

「不用完全参照,他的色彩太暗了,但是布局和层级结构可以考虑」

## 2. 校正范围

### ✅ 借鉴 spacemit 的部分(主 spec 节 1 + 节 4 + 节 6 适用)

- **layout pattern**:hero 全宽 / 卡片 grid / nav 结构 / footer 多 column
- **信息层级**:badge → title → highlight → description → CTA 四层 hero 排布
- **组件 hierarchy**:button(primary 强 + secondary 弱)/ card(elevation 1-3)/ chip / badge
- **页面骨架**:home → product → solution → docs → blog → contact 六大主入口
- **nav menu pattern**:hover dropdown / sticky top / 双 CTA 突出 download + sales

### ❌ 不借鉴 spacemit 的部分(主 spec 节 3 design tokens 配色覆盖)

- **不要**深灰 / 暗蓝主背景
- **不要**低对比度 typography
- **不要**整体偏冷色调

## 3. engi-stack 明亮配色 spec(覆盖主 spec §3 colors 部分)

```yaml
# engi-stack 配色 — 明亮路线(覆盖 spec §3)
colors:
  primary:                       # 主色 — 选 1 个明亮 accent
    50:  "#f0fdfa"               # 极浅(badge bg / soft surface)
    100: "#ccfbf1"
    300: "#5eead4"
    500: "#14b8a6"               # 主 accent — Teal 500(明亮但不刺眼)
    600: "#0d9488"               # hover state
    700: "#0f766e"               # active / dark text
    900: "#134e4a"               # 深色仅用于极强 emphasis
  # 备选主色(user 可选):
  #   - emerald 500 #10b981(绿,更生命力)
  #   - violet  500 #8b5cf6(紫,更 AI 感)
  #   - sky     500 #0ea5e9(蓝,但比 spacemit 亮)
  #   - amber   500 #f59e0b(橙,温暖)

  background:
    primary:   "#ffffff"          # 主背景 white
    secondary: "#f8fafc"          # 卡片 / section 微底色(slate-50)
    tertiary:  "#f1f5f9"          # hover bg(slate-100)
    dark:      "#0f172a"          # 仅 dark mode(后续可选,v1.0 默认 light only)

  text:
    primary:   "#0f172a"          # 主文本 near-black(slate-900,对比度 16:1)
    secondary: "#475569"          # 次要文本(slate-600,对比度 7:1)
    tertiary:  "#94a3b8"          # placeholder / disabled(slate-400)
    inverse:   "#ffffff"          # 主色按钮上的文本

  border:
    light:   "#e2e8f0"            # slate-200(默认 border)
    medium:  "#cbd5e1"            # slate-300(hover)
    accent:  "#14b8a6"            # 强调 border(primary 500)

  status:
    success: "#10b981"
    warning: "#f59e0b"
    danger:  "#ef4444"
    info:    "#3b82f6"
```

**对比 spacemit**:
| 项 | spacemit | engi-stack |
|----|---------|-----------|
| 主背景 | 深灰 / 黑 | white / slate-50 |
| 主文本 | 浅灰(对比度低) | slate-900(对比度高)|
| 主色 | 蓝灰(偏暗) | teal 500(明亮 accent) |
| 整体感觉 | 工业 / 冷峻 | 现代 / 清爽 / 亲和 |

## 4. typography 微调(覆盖主 spec §3 typography 部分)

不变化字体族(Inter + JetBrains Mono),但**字重 / 对比度**:

- hero title 用 `font-weight: 700`(spacemit 用 600,我们略强调以补偿明亮背景)
- body text 主用 `slate-700` 而非 `slate-900`(避免黑底白字过强对比)
- secondary text `slate-500`(spacemit 用 `slate-400` 太低)

## 5. 实施先后

a0cbb49 主 spec 落档后,合并到主 spec:
- §3 colors 完全替换为本 amend 配色
- §3 typography 字重 / 对比度 微调
- §11 风险 R1 改:已差异化(明亮 teal vs spacemit 暗蓝灰),无品牌混淆
- 节 4 组件保持 spacemit 借鉴(layout + hierarchy)

## 6. user 决策点(主色)

明亮系 4 候选,user 选 1:

| 选项 | 色值 | 暗示 |
|------|------|------|
| **A. Teal 500** ⭐(默认) | `#14b8a6` | 现代 / 科技 / 知识 |
| B. Emerald 500 | `#10b981` | 生机 / 隐私 / 自然 |
| C. Violet 500 | `#8b5cf6` | AI 感 / 创意 / 未来 |
| D. Sky 500 | `#0ea5e9` | 平静 / 信任 / 干净(但 spacemit 偏蓝,差异度低)|

A 默认推荐(与 spacemit 蓝灰差异度最大 + 明亮 + AI/知识工具适配)。

## 7. 与 a34d92b0 official-web 内容 agent 衔接

a34d92b0 在写 `branding.yaml`(产品元数据 / nav / footer 内容),与本 design tokens **正交**:
- branding.yaml = WHAT(品牌名 / logo path / brand tagline / nav 结构)
- design tokens = HOW(颜色 / 字体 / 间距 / 圆角)

a34d92b0 完成后,实施 sprint(推 v1.0.3 与 Observability 配合 visual regression):
- WordPress `themes/engi-stack/style.css` 用 CSS custom properties
- branding.yaml 引 `themes.primary_color: var(--color-primary-500)` 等

## 8. 实施时机

- v1.0.0(today)+ v1.0.1(5/26-28):**主 spec + 本 amend 落档**(已完成本 amend)
- v1.0.2(5/31):primary 色 user 选定 + 主 spec section 3 合并 amend
- v1.0.3(6/05):**4 表面 design tokens 同步实施**(配合 Observability sprint visual regression test)
- v1.0.9(7/15):**i18n 切换契约真验证**(与 i18n sprint 同期)
