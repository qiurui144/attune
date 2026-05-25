# UI 视觉统一 — D+A 双色组合(Sky + Teal)落地 spec

> v2 amend(2026-05-25 15:00)— user 选 **D + A 组合**(Sky `#0ea5e9` + Teal `#14b8a6`),覆盖 amend v1 的"4 候选选 1"。

## 1. 用户原话

「D+A,看看如何能实现设计」

## 2. 配色系统(Dual-tone)

**Primary brand**: **Teal 500 `#14b8a6`** — 品牌主色 / Logo / 主 CTA / 主 active state
**Secondary accent**: **Sky 500 `#0ea5e9`** — 链接 / info / 渐变 / 次 CTA

为什么是 analogous palette:
- 色相相邻(Sky 195° → Teal 173°),天然 harmony
- 共同主题:水 / 天空 / 自然 → 暗示「私有 AI 知识伙伴」(平静 + 智能 + 流动)
- 中高饱和,与 spacemit 低饱和蓝灰区分明确

## 3. 完整 token spec

```css
/* engi-stack design tokens v1.0(覆盖 amend v1 §3 colors) */
:root {
  /* ===== Primary: Teal(品牌主色) ===== */
  --color-primary-50:  #f0fdfa;  /* badge bg / soft surface */
  --color-primary-100: #ccfbf1;  /* hover soft */
  --color-primary-300: #5eead4;  /* gradient lighter stop */
  --color-primary-500: #14b8a6;  /* ⭐ 主色 */
  --color-primary-600: #0d9488;  /* hover state */
  --color-primary-700: #0f766e;  /* active state */
  --color-primary-900: #134e4a;  /* dark emphasis */

  /* ===== Secondary: Sky(链接 / 渐变 / 次 CTA) ===== */
  --color-secondary-50:  #f0f9ff;
  --color-secondary-100: #e0f2fe;
  --color-secondary-300: #7dd3fc;  /* gradient lighter stop */
  --color-secondary-500: #0ea5e9;  /* ⭐ 次色 */
  --color-secondary-600: #0284c7;  /* link hover */
  --color-secondary-700: #0369a1;  /* active state */

  /* ===== Backgrounds ===== */
  --bg-primary:   #ffffff;
  --bg-secondary: #f8fafc;  /* section bg / card */
  --bg-tertiary:  #f1f5f9;  /* hover bg */

  /* ===== Text ===== */
  --text-primary:   #0f172a;  /* slate-900,16:1 contrast on white */
  --text-secondary: #475569;  /* slate-600,7:1 */
  --text-tertiary:  #94a3b8;  /* slate-400 disabled */
  --text-inverse:   #ffffff;
  --text-link:      var(--color-secondary-600);  /* Sky 600 易识别为链接 */

  /* ===== Borders ===== */
  --border-light:  #e2e8f0;
  --border-medium: #cbd5e1;
  --border-accent: var(--color-primary-500);

  /* ===== Status ===== */
  --status-success: #10b981;
  --status-warning: #f59e0b;
  --status-danger:  #ef4444;
  --status-info:    var(--color-secondary-500);
}
```

## 4. 双色应用模式(具体 7 场景)

### 4.1 Logo(Teal 主 + Sky highlight)

```
[engi-stack]
  ↑           ↑
  Teal 500    Sky 500(可选高亮"i"或"-")
```

简化(单色):全 Teal 500。
双色:`stack` 中的 `s` 或 `t` 用 Sky 500 点缀。

### 4.2 Hero gradient(Sky → Teal)

```css
.hero {
  background: linear-gradient(135deg, 
    var(--color-secondary-500) 0%,    /* Sky 500 左上 */
    var(--color-primary-500) 100%      /* Teal 500 右下 */
  );
  color: var(--text-inverse);
}

/* 软化版(白底为主,gradient 仅 hero box)*/
.hero-soft {
  background: linear-gradient(135deg,
    var(--color-secondary-50) 0%,     /* Sky 50 极浅 */
    var(--color-primary-50) 100%       /* Teal 50 极浅 */
  );
  color: var(--text-primary);
}
```

### 4.3 Primary CTA(Teal 500 bg + white)

```css
.btn-primary {
  background: var(--color-primary-500);
  color: var(--text-inverse);
  border: none;
}
.btn-primary:hover {
  background: var(--color-primary-600);
}
```

### 4.4 Secondary CTA(White bg + Sky border + Sky text)

```css
.btn-secondary {
  background: var(--bg-primary);
  color: var(--color-secondary-600);
  border: 1.5px solid var(--color-secondary-500);
}
.btn-secondary:hover {
  background: var(--color-secondary-50);
  border-color: var(--color-secondary-600);
}
```

### 4.5 Link / inline accent(Sky 600 text)

```css
a {
  color: var(--text-link);  /* Sky 600 */
  text-decoration: underline;
  text-decoration-color: var(--color-secondary-300);
  text-underline-offset: 3px;
}
a:hover {
  color: var(--color-secondary-700);
  text-decoration-color: var(--color-secondary-600);
}
```

### 4.6 Section accent(Teal 50 soft surface)

```css
.section-highlight {
  background: var(--color-primary-50);  /* 极浅 teal,易区分但不喧宾夺主 */
  border-left: 3px solid var(--color-primary-500);
  padding: 1.5rem;
}
```

### 4.7 Data viz / charts(双色 + 拓展)

```css
.chart-color-1 { color: var(--color-primary-500); }   /* Teal */
.chart-color-2 { color: var(--color-secondary-500); } /* Sky */
.chart-color-3 { color: var(--color-primary-300); }   /* Teal light */
.chart-color-4 { color: var(--color-secondary-300); } /* Sky light */
/* 补充第 5+ 系列时用 status 色或 complement(amber #f59e0b)*/
```

## 5. 跨表面同步策略(4 user-facing surfaces)

### 5.1 SSOT 文件

`cloud/design-tokens/tokens.yaml`(待 v1.0.3 创建):

```yaml
# SSOT — 所有表面 build-time 拉取
primary:
  500: "#14b8a6"
  # ... (全 ramp)
secondary:
  500: "#0ea5e9"
  # ... (全 ramp)
# ... (其他 token)
```

### 5.2 各表面 transformer 脚本

| 表面 | 输出 | transformer |
|------|------|-------------|
| official-web(WP) | `themes/engi-stack/css/tokens.css` + theme.json | yaml → css custom properties + WP customizer 字段 |
| wiki-web(Docusaurus) | `src/css/tokens.css` + `swizzled/Footer/styles.module.css` | yaml → css + JS export |
| accounts(Django) | `static/css/tokens.css` | yaml → css |
| attune-server UI(React) | `ui/src/design-tokens.ts` | yaml → TS const + Tailwind config |

build-time(non-runtime)同步,**0 性能开销**。

### 5.3 跨表面一致性自动测试(推 v1.0.3)

CI 跑 `scripts/verify-design-tokens-sync.sh`:
- grep `#14b8a6` 在 4 表面 CSS / TS / JSON 都出现
- 任一表面 hardcode `rgb(...)` 不命中 SSOT → fail

## 6. 与 spacemit 布局借鉴对照表

| spacemit 借鉴项 | engi-stack 应用 |
|---------------|----------------|
| Hero 全宽 + 中央 CTA + 右侧 illustration | ✅ 直接借鉴 |
| 4 列产品卡片 grid(等高,hover 抬起) | ✅ 直接借鉴 |
| Sticky top nav + hover dropdown | ✅ 直接借鉴 |
| Footer 3 column(产品 / 社区 / 法律) | ✅ 直接借鉴 |
| 信息层级:badge → title → highlight → description → CTA | ✅ 直接借鉴 |
| Section 间距大(80-120px) | ✅ 直接借鉴 |
| 图标使用(Lucide / Heroicons,outline 风格) | ✅ 直接借鉴 |
| **配色**:深灰背景 + 蓝灰 accent + 低对比文本 | ❌ **不借鉴** — engi-stack 走 white + Sky/Teal + 高对比 |
| **字体**:细字重 + 偏小 | ❌ **不借鉴** — engi-stack hero 700 字重 + slate-900 文本 |

## 7. mockup HTML 可视化(立即可看)

落档 `docs/screenshots/v1-0-ui-mockups/hero-dual-tone.html` — user 浏览器打开看效果。

(详 mockup 文件 separate commit)

## 8. 实施时间表

| 阶段 | 内容 | 时机 |
|------|------|------|
| **本 amend v2** | 双色 spec + mockup HTML(view-only) | **2026-05-25** today |
| spec 合并 | a0cbb49 base spec + amend v1 + amend v2 合并到主 spec | a0cbb49 完成后 |
| SSOT tokens.yaml | 创建 cloud/design-tokens/ + 4 transformer | v1.0.3(6/05) |
| 4 表面实施 | official-web theme / wiki theme / accounts CSS / attune-server TS | v1.0.3 |
| visual regression test | Playwright + visual diff | v1.0.3 |
| user 实测 + 调整 | 真部署后 user 体验调整 | v1.0.4+ |

## 9. 实施 user 决策点

- Q1: Logo 是否要重新设计?(若纯文字 logo,只换色简单;若图形 logo 需 design)
- Q2: hero gradient 倾向 **strong**(Sky → Teal 满色)还是 **soft**(Sky 50 → Teal 50 浅色)?
- Q3: 是否要 dark mode?(v1.0 默认 light only,dark mode 推 v1.1)
- Q4: 字体是否要付费 brand font?(默认免费 Inter + JetBrains Mono)

## 10. 风险

| R | 描述 | 缓解 |
|---|------|------|
| R1 | Sky + Teal 与某些 brand 冲突(如 Stripe 用 #635bff 紫,Twilio 用红) | 当前调研显示无主流 SaaS 用 Sky+Teal 双色组合,差异化足够 |
| R2 | 双色应用过度,视觉混乱 | 严守 7 场景应用规则;主色 80%,次色 15%,中性 100%(背景)|
| R3 | 渐变 hero 与 white background 主体反差大,视觉跳 | 用 soft 渐变版(Sky 50 → Teal 50)+ 主色仅在 hero 文字 / CTA |
| R4 | Sky 500 与某些 status info 撞色 | status info 改用 Sky 600(稍深),区分链接 vs 系统消息 |
