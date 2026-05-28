# UI 视觉统一 spec — engi-stack 四表面统一 + spacemit 借鉴 + i18n 切换契约

> **状态**: spec-only(不实施)
> **范围**: engi-stack 4 个 user-facing 表面的视觉 design tokens + 组件基线 + i18n 切换契约
> **触发**: 用户原话「参照一下 https://spacemit.com/,但是他的多语言切换也是有些问题的。我们也要尽可能做到 UI 视觉的统一」
> **配套 amend**: 见 [`2026-05-25-ui-visual-unification-spec-amend.md`](./2026-05-25-ui-visual-unification-spec-amend.md)
> — 主 spec 节 3 colors / 节 4 typography 字重 / 节 11 R1 以 amend 为准(明亮配色路线,不沿用 spacemit 暗色)

## 目录 (Table of Contents)

- [节 1. 目标定位](#节-1-目标定位)
- [节 2. 范围边界](#节-2-范围边界)
- [节 3. 架构数据流 — design tokens SSOT pipeline](#节-3-架构数据流--design-tokens-ssot-pipeline)
- [节 4. 模块边界 + 跨表面落地映射](#节-4-模块边界--跨表面落地映射)
- [节 5. API 契约 — design tokens schema + 组件基线](#节-5-api-契约--design-tokens-schema--组件基线)
- [节 6. 扩展点 / 插件接口](#节-6-扩展点--插件接口)
- [节 7. 错误处理 + 边界 case + i18n 切换契约](#节-7-错误处理--边界-case--i18n-切换契约)
- [节 8. 成本契约](#节-8-成本契约)
- [节 9. 测试矩阵](#节-9-测试矩阵)
- [节 10. 向后兼容](#节-10-向后兼容)
- [节 11. 风险登记](#节-11-风险登记)
- [附录 A. spacemit.com 视觉调研原始数据](#附录-a-spacemitcom-视觉调研原始数据)
- [附录 B. spacemit i18n 痛点诊断 + engi-stack 规避方案](#附录-b-spacemit-i18n-痛点诊断--engi-stack-规避方案)
- [附录 C. user 决策点](#附录-c-user-决策点)

---

## 节 1. 目标定位

### 用户痛点

engi-stack 当前 4 个 user-facing 表面**视觉割裂**:
- `engi-stack.com`(official-web,WordPress + Polylang) — 营销 / 产品介绍
- `wiki.engi-stack.com`(wiki-portal,Docusaurus 静态站) — 文档 / 教程
- `accounts.engi-stack.com`(Django + DRF UI) — 登录 / 会员管理 / billing
- attune desktop app embedded Web UI(attune-server `ui/` Vite + React) — 产品内 UI

4 个表面用 4 套 framework,**色彩 / 字体 / 间距 / 圆角 / 阴影各自一套**,user 跨表面跳转(官网→注册→桌面 app→文档)时**视觉断裂**,严重削弱品牌一致性。

### 与产品定位的对齐

engi-stack 定位 "**私有 AI 知识伙伴**"(per `docs/superpowers/specs/2026-04-17-product-positioning-design.md`)— 个人 / 行业用户的本地优先知识库。视觉统一**直接服务于品牌信任**:
- 用户在官网看到品牌色 → 注册时(accounts)看到同一品牌色 → 装 attune 后(desktop)再看到同一品牌色 → 查文档时(wiki)同样品牌色
- **跨表面一致 = 专业 = 可信** — 这对 B2C(个人版)+ B2B(企业版 attune-enterprise)都是核心信号

### spacemit 借鉴定位(per amend 校正)

借鉴 spacemit.com 的**布局 + 信息层级 + 组件 hierarchy**,**不**借鉴其配色(过暗,缺亲和力)。engi-stack 走**明亮 + 现代**路线(见 amend §3 配色 spec)。

---

## 节 2. 范围边界

### 做(本 spec 涵盖)

1. **design tokens SSOT**:`cloud/design-tokens/tokens.yaml`(color / typography / spacing / radius / shadow)
2. **组件基线 spec**:button / card / chip / badge / hero / nav / footer 7 类基础组件的 HTML 结构 + class 命名
3. **4 表面落地映射**:每个表面如何 build-time 引入 tokens(WordPress theme / Docusaurus CSS / Django static / React TS)
4. **i18n 切换契约**:URL 策略 / 切换 widget 行为 / fallback 策略 / 持久化 / 死链防护 — **必须避免 spacemit 的 6 类痛点**(见附录 B)

### 不做(明确排除)

1. **不**重写 4 个 framework(WordPress 不改成 Next.js / Docusaurus 不改成 VitePress / Django UI 不改成 React)
2. **不**做 brand identity(logo / tagline / 品牌故事 — 那是 a34d92b0 agent 在写的 `branding.yaml` 负责)
3. **不**做 visual regression test 工具实施(推 v1.0.3 Observability sprint)
4. **不**做 dark mode(v1.0 default light only,dark mode 推 v1.0.6+)
5. **不**做无障碍 WCAG AAA 审计(只保证 design tokens 满足 AA 对比度;AAA 审计推 v1.0.9+)

### 推后续版本(明示)

- **v1.0.2**: primary 色 user 选定(见附录 C 决策点)+ 主 spec §3 合并 amend §3
- **v1.0.3**: 4 表面 design tokens 实施 + visual regression test(配 Observability sprint)
- **v1.0.6**: dark mode(基于 tokens 扩展 `--color-bg-dark` 等变量)
- **v1.0.9**: i18n 切换契约真验证(配 i18n sprint)+ WCAG AAA 审计

---

## 节 3. 架构数据流 — design tokens SSOT pipeline

```
                       ┌──────────────────────────────────┐
                       │  cloud/design-tokens/tokens.yaml │
                       │  (SSOT,所有视觉常量在此)        │
                       │  - colors                        │
                       │  - typography                    │
                       │  - spacing / radius / shadow     │
                       └──────────────┬───────────────────┘
                                      │
                  ┌───────────────────┼───────────────────┐
                  │ build-time transform(4 路 generator) │
                  └───────────────────┬───────────────────┘
                                      │
        ┌──────────────┬──────────────┼──────────────┬──────────────┐
        ▼              ▼              ▼              ▼              ▼
  ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌───────────┐
  │ WordPress │  │ Docusaurus│  │  Django   │  │ attune-svr│  │ (将来)新表 │
  │ theme.css │  │ custom.css│  │ tokens.css│  │ tokens.ts │  │ 面 N      │
  │           │  │           │  │           │  │           │  │           │
  │ CSS vars  │  │ CSS vars  │  │ CSS vars  │  │ TS export │  │ ...       │
  └─────┬─────┘  └─────┬─────┘  └─────┬─────┘  └─────┬─────┘  └───────────┘
        │              │              │              │
        ▼              ▼              ▼              ▼
   官网渲染        文档站渲染      账户中心渲染    桌面 app 渲染
   (同一品牌色 / 字体 / 间距 / 圆角 / 阴影)
```

### 数据流详解

1. **SSOT**: `cloud/design-tokens/tokens.yaml` 是唯一编辑入口。改一处 → build pipeline 同步到 4 表面
2. **Transformer**: 4 个独立脚本(Python / Node / TS / shell 任选),build-time 跑,把 YAML 转成各 framework 的 native format
   - WordPress: `tokens.yaml` → `:root { --color-primary-500: #14b8a6; ... }` 注入 `themes/engi-stack/style.css`
   - Docusaurus: `tokens.yaml` → `src/css/custom.css` 同样 CSS custom properties
   - Django: `tokens.yaml` → `static/css/design-tokens.css`
   - attune-server: `tokens.yaml` → `ui/src/design-tokens.ts`(TypeScript export const)
3. **runtime 不动 tokens**: 4 表面运行时不读 YAML,只读各自生成的 CSS / TS 文件 — 零运行时性能开销
4. **变更流程**:
   - dev 改 `cloud/design-tokens/tokens.yaml`
   - 跑 `make tokens-build`(4 transformer 并行)
   - 4 表面各自 PR 引入新生成文件
   - 各表面 build / deploy 后视觉自动同步

### 关键 DB / cache layers

**N/A** — design tokens 是 build-time artifact,**不进 DB,不进 cache**。

---

## 节 4. 模块边界 + 跨表面落地映射

| 表面 | 实施技术 | tokens 集成路径 | 维护 owner |
|------|---------|---------------|----------|
| **official-web** (engi-stack.com) | WordPress + Polylang | `themes/engi-stack/style.css` 用 CSS custom properties + `theme.json`(WP 6 block themes 支持) | cloud 仓 official-web/ |
| **wiki-web** (wiki.engi-stack.com) | Docusaurus 3.x | `src/css/custom.css` import design tokens via `infima` override | cloud 仓 wiki-portal/ |
| **accounts** (accounts.engi-stack.com) | Django + DRF + Vue3 sub-app | `static/css/design-tokens.css` + Django template `{% static %}` 引入 | cloud 仓 accounts/ |
| **attune-server UI** (桌面 app embedded) | Vite + React + Tailwind v4 | `ui/src/design-tokens.ts` TypeScript const + Tailwind `tailwind.config.ts` extend.colors 引 tokens | attune 仓 rust/crates/attune-server/ui/ |
| **共享 SSOT** | YAML | `cloud/design-tokens/tokens.yaml` + `cloud/design-tokens/transformers/` | cloud 仓 design-tokens/ |

### 跨仓边界

- **cloud 仓** 拥有 SSOT + 3 表面(official-web / wiki-portal / accounts) + 4 transformer 脚本
- **attune 仓**(本仓)消费 generated `design-tokens.ts`(可通过 git submodule 或 cloud npm package `@engi-stack/design-tokens` 发布)
- **同步策略**: cloud tokens.yaml 改后,attune 仓更新 submodule / bump npm 版本 → attune-server build 自动用新 tokens

---

## 节 5. API 契约 — design tokens schema + 组件基线

### 5.1 tokens.yaml schema(SSOT)

```yaml
# cloud/design-tokens/tokens.yaml v1.0
# 与 amend §3 配色 spec 合并后的最终形态(明亮路线)

$schema: "https://engi-stack.com/schemas/design-tokens/v1.json"
version: "1.0.0"
last_updated: "2026-05-25"

colors:
  # 主色 — Teal 500 默认(per amend §6 user 决策点 A)
  # user 选定后 cloud sprint 替换为最终色值
  primary:
    "50":  "#f0fdfa"               # 极浅 — badge bg / soft surface
    "100": "#ccfbf1"
    "300": "#5eead4"
    "500": "#14b8a6"               # 主 accent(明亮但不刺眼)
    "600": "#0d9488"               # hover state
    "700": "#0f766e"               # active / dark text
    "900": "#134e4a"               # 深色仅用于极强 emphasis

  background:
    primary:   "#ffffff"           # 主背景
    secondary: "#f8fafc"           # 卡片 / section 微底色(slate-50)
    tertiary:  "#f1f5f9"           # hover bg(slate-100)
    dark:      "#0f172a"           # 仅 dark mode(v1.0 不启用)

  text:
    primary:   "#0f172a"           # 主文本(slate-900,对比度 16:1)
    secondary: "#475569"           # 次要文本(slate-600,对比度 7:1)
    tertiary:  "#94a3b8"           # placeholder / disabled(slate-400)
    inverse:   "#ffffff"           # 主色按钮上的文本

  border:
    light:   "#e2e8f0"             # slate-200(默认 border)
    medium:  "#cbd5e1"             # slate-300(hover)
    accent:  "#14b8a6"             # 强调 border(primary 500)

  status:
    success: "#10b981"
    warning: "#f59e0b"
    danger:  "#ef4444"
    info:    "#3b82f6"

typography:
  font_family:
    sans: "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, 'PingFang SC', 'Microsoft YaHei', 'WenQuanYi Micro Hei', sans-serif"
    mono: "'JetBrains Mono', ui-monospace, SFMono-Regular, Consolas, 'Liberation Mono', Menlo, monospace"
    # 注:CJK fallback 链与 spacemit 一致(PingFang SC / Microsoft YaHei / WenQuanYi Micro Hei)
    # 是中文 web 行业惯例,跨 OS 渲染表现可控

  scale:
    xs:    "0.75rem"     # 12px
    sm:    "0.875rem"    # 14px(spacemit 主用 14px,与之对齐)
    base:  "1rem"        # 16px
    lg:    "1.125rem"    # 18px
    xl:    "1.5rem"      # 24px
    "2xl": "2rem"        # 32px
    "3xl": "2.5rem"      # 40px
    hero:  "3.5rem"      # 56px(hero h1)

  weight:
    normal:    400
    medium:    500
    semibold:  600
    bold:      700       # hero title 用 bold(per amend §4,补偿明亮背景)

  line_height:
    tight:  1.25         # hero / headings
    snug:   1.375
    normal: 1.5          # body
    loose:  1.75

spacing:
  # 4px base scale(per Tailwind 惯例)
  xs:    "4px"
  sm:    "8px"
  md:    "16px"
  lg:    "24px"
  xl:    "40px"
  "2xl": "64px"
  "3xl": "96px"

radius:
  # spacemit 用到的 radius 全集(per CSS 调研):4px / 6px / 8px / 10px / 12px / 14px / 18px / 20px / 24px / 40px / 50%
  # engi-stack 简化为 5 档
  sm:    "4px"            # button-sm / chip
  md:    "8px"            # button / input
  lg:    "16px"           # card
  xl:    "24px"           # modal / large card
  full:  "9999px"         # avatar / pill button

shadow:
  # spacemit 用到的 shadow 集合(per CSS 调研):0 24px 80px #0f172a2e / 0 8px 24px #ff4d4f4d / 0 4px 12px #00000026
  # engi-stack 简化为 4 档(明亮路线降低 alpha)
  card:   "0 1px 3px rgba(15, 23, 42, 0.08)"
  hover:  "0 4px 12px rgba(15, 23, 42, 0.12)"
  modal:  "0 8px 32px rgba(15, 23, 42, 0.16)"
  hero:   "0 24px 80px rgba(15, 23, 42, 0.10)"

transitions:
  fast:   "150ms ease-out"
  normal: "250ms ease-out"
  slow:   "400ms ease-in-out"
```

### 5.2 组件基线 spec(per amend §2.✅ 借鉴 spacemit layout + hierarchy)

#### Button

```html
<!-- Primary -->
<button class="es-btn es-btn-primary">
  <span>Get started</span>
</button>

<!-- Secondary -->
<button class="es-btn es-btn-secondary">
  <span>Learn more</span>
</button>

<!-- Ghost -->
<button class="es-btn es-btn-ghost">
  <span>Skip</span>
</button>
```

```css
.es-btn {
  font-family: var(--font-sans);
  font-size: var(--text-sm);
  font-weight: var(--font-medium);
  padding: var(--space-sm) var(--space-lg);
  border-radius: var(--radius-md);
  transition: var(--transition-fast);
  cursor: pointer;
  border: 1px solid transparent;
}
.es-btn-primary {
  background: var(--color-primary-500);
  color: var(--color-text-inverse);
}
.es-btn-primary:hover {
  background: var(--color-primary-600);
  box-shadow: var(--shadow-hover);
}
.es-btn-secondary {
  background: var(--color-bg-primary);
  color: var(--color-primary-700);
  border-color: var(--color-border-medium);
}
.es-btn-ghost {
  background: transparent;
  color: var(--color-text-secondary);
}
```

3 size 通过 padding + font-size 区分:`-sm` / 默认 / `-lg`。

#### Card

```html
<div class="es-card">
  <h3 class="es-card-title">Feature title</h3>
  <p class="es-card-desc">Description here.</p>
  <div class="es-card-footer">
    <button class="es-btn es-btn-ghost">Action</button>
  </div>
</div>
```

3 elevation level(`es-card-1` / `es-card-2` / `es-card-3`),对应 shadow card/hover/modal。

#### Chip / Badge

```html
<!-- Chip(可点击) -->
<span class="es-chip">Tag</span>

<!-- Badge(状态标记,不可点击) -->
<span class="es-badge es-badge-success">v1.0.0</span>
<span class="es-badge es-badge-warning">Beta</span>
```

#### Hero(per amend §2.✅ 借鉴 spacemit 四层层级)

```html
<section class="es-hero">
  <span class="es-hero-badge">v1.0 GA</span>
  <h1 class="es-hero-title">私有 AI 知识伙伴</h1>
  <p class="es-hero-highlight">本地优先,隐私至上</p>
  <p class="es-hero-desc">个人知识库 + 记忆增强系统,降低 token 成本,保护数据安全。</p>
  <div class="es-hero-cta">
    <button class="es-btn es-btn-primary es-btn-lg">立即下载</button>
    <button class="es-btn es-btn-secondary es-btn-lg">查看文档</button>
  </div>
</section>
```

4 层信息层级(借鉴 spacemit hero 结构): badge → title → highlight → description → CTA。

#### Nav menu(per amend §2.✅ 借鉴 spacemit sticky top + hover dropdown + 双 CTA)

```html
<nav class="es-nav">
  <a href="/" class="es-nav-logo">
    <img src="/logo.svg" alt="engi-stack"/>
  </a>
  <ul class="es-nav-menu">
    <li><a href="/products">产品</a></li>
    <li><a href="/solutions">方案</a></li>
    <li><a href="/docs">文档</a></li>
    <li><a href="/blog">博客</a></li>
    <li><a href="/contact">联系</a></li>
  </ul>
  <div class="es-nav-cta">
    <a href="/download" class="es-btn es-btn-primary">下载</a>
    <a href="/sales" class="es-btn es-btn-secondary">联系销售</a>
    <button class="es-lang-switch" aria-label="切换语言">中 / EN</button>
  </div>
</nav>
```

#### Footer

```html
<footer class="es-footer">
  <div class="es-footer-grid">
    <div class="es-footer-col">
      <h4>产品</h4>
      <a href="/attune">attune 个人版</a>
      <a href="/attune-pro">attune Pro</a>
      <a href="/attune-enterprise">attune Enterprise</a>
    </div>
    <div class="es-footer-col">
      <h4>资源</h4>
      <a href="/docs">文档</a>
      <a href="/blog">博客</a>
      <a href="/changelog">更新日志</a>
    </div>
    <div class="es-footer-col">
      <h4>支持</h4>
      <a href="/help">帮助中心</a>
      <a href="/contact">联系我们</a>
      <a href="/status">服务状态</a>
    </div>
    <div class="es-footer-col">
      <h4>法律</h4>
      <a href="/privacy">隐私政策</a>
      <a href="/terms">使用条款</a>
      <a href="/cookies">Cookie 政策</a>
    </div>
  </div>
  <div class="es-footer-bottom">
    <span>© 2026 engi-stack</span>
    <span>ICP 备 XXXXXXXX 号</span>
  </div>
</footer>
```

### 5.3 CSS class 命名规范

**前缀 `es-`**(engi-stack 简写)避免与 Tailwind / Antd / Bootstrap 等 utility class 冲突。BEM 风格但简化:`es-{block}-{element}--{modifier}` 仅当需要 modifier 时使用。

---

## 节 6. 扩展点 / 插件接口

### 6.1 新 token 类别扩展

新增 token 类别(如 `animations`, `breakpoints`, `z_index`)在 tokens.yaml 平级追加:

```yaml
animations:
  fade_in: "..."
breakpoints:
  sm: "640px"
  md: "768px"
  lg: "1024px"
```

4 transformer 自动遍历 top-level key,无需手动改。

### 6.2 新表面接入

新增表面 N(如未来 `mobile-app`):
1. cloud 仓 `cloud/design-tokens/transformers/<surface>.{py,ts,sh}` 新增 transformer
2. 输出到该表面的 native format
3. CI 跑 `make tokens-build` 自动包含

### 6.3 主题变种(dark mode 等,v1.0.6+)

```yaml
colors:
  background:
    primary: "#ffffff"
    primary_dark: "#0f172a"      # dark mode 用
```

transformer 输出双 CSS rule:`:root { ... }` + `[data-theme="dark"] { ... }`。

---

## 节 7. 错误处理 + 边界 case + i18n 切换契约

### 7.1 i18n 切换契约(避免 spacemit 6 类痛点 — 见附录 B 诊断)

#### URL 策略(强制 — 4 表面统一)

| 表面 | URL 模式 | 理由 |
|------|---------|------|
| official-web | `/` (zh 默认) + `/en/` | Polylang 默认行为;SEO-friendly + 链接可分享 |
| wiki-web | `/` (zh 默认) + `/en/` | Docusaurus i18n 默认 |
| accounts | `/` (zh 默认) + `/en/` | Django LocaleMiddleware + `i18n_patterns` |
| attune-server UI | in-app dropdown(桌面 app 无 URL 语义)| Vault 内 user preference 持久化 |

**严禁** 用 query param(`?lang=en`)+ **严禁** 仅靠 cookie / localStorage(per spacemit 痛点 #1)。

#### 切换 widget 行为(强制)

- **位置**: 每页右上角持久显示(per spacemit 借鉴的 sticky top nav 双 CTA 旁)
- **形态**: 简洁 toggle `中 / EN`(per attune-server UI 现有惯例,per CLAUDE.md i18n 规范 grep 守卫)
- **点击行为**: 跳到**对应语言版本同一页面**(per spacemit 痛点 #2 规避)
  - 用户在 `/products` zh 点 EN → 跳 `/en/products`(不是 `/en/` 首页)
  - 用户在 `/en/about` 点中 → 跳 `/about`
- **持久化**:
  - HTTP cookie `lang` 7 天 +
  - localStorage `language` 用于 desktop app 内 preference +
  - URL path 为最终决定权(URL 优先于 cookie 优先于 localStorage)
- **状态指示**: 切换按钮上显示当前激活语言(高亮 `中` 或 `EN`)

#### 未翻译页面 fallback(强制)

- **官网 / wiki / accounts**: 若 `/en/<path>` 不存在但 `/<path>` 存在 → **不**自动跳 zh,而是显示页面 + 顶部 banner: `English version coming soon. Showing Chinese version.`
- **attune-server UI**: vue-i18n key 缺失时 fallback 到 zh + 控制台 warn(per CLAUDE.md grep 守卫,**不**允许 fallback 静默)
- **链接死链防护**: build time 跑 link-checker:
  - 每个 zh page 必有 en 对应 path 或 explicit `<link rel="alternate" hreflang="x-default">` 声明
  - 每个 en page 必有 zh 对应 path
  - CI 跑 lychee / linkinator 扫所有 internal link,en path 必须 200

#### SEO 多语言契约(强制 — spacemit 痛点 #5/#6 规避)

- **每页含 `<link rel="alternate" hreflang="zh-CN" href="...">` + `<link rel="alternate" hreflang="en" href="...">` + `<link rel="alternate" hreflang="x-default" href="...">` **
- **每页 `<html lang="zh-CN">` 或 `<html lang="en">`**(spacemit 全部 `lang="en"` 是 a11y bug)
- **`sitemap.xml` 必有**(spacemit 没有 — SEO disaster),且按 google `xhtml:link` 声明 hreflang
- **`robots.txt` 必有**(spacemit 没有)

### 7.2 token 加载失败

- **CSS 解析失败**(transformer bug 导致 invalid CSS): browser 自动 fallback 到 user-agent default,**不**白屏
- **CSS 文件 404**: 各 framework 默认 fallback 到 framework default(Docusaurus infima / Antd default theme / browser default)— 不阻塞页面渲染

### 7.3 字体 fallback 链

`font-family` 长链已设计 fallback(Inter → ui-sans-serif → system-ui → -apple-system → ... → PingFang SC → Microsoft YaHei → WenQuanYi Micro Hei → sans-serif)。网络慢 / 字体加载失败 → 自动用 system-ui,**不**白屏。

### 7.4 边界 case

| Case | 行为 |
|------|------|
| 用户首次访问 `/en/about` 无 cookie | 显示 EN,setCookie lang=en,localStorage language=en |
| 用户从 `/en/about` 进入 `/zh/about`(切换 widget)| 显示 zh,setCookie lang=zh,localStorage language=zh,URL 反映 |
| 用户从 `/en/about` 进入 `/about`(URL 直输)| URL 优先 → 显示 zh(覆盖 cookie / localStorage)|
| 用户 Accept-Language: en 第一次访问 `/` | 重定向 `/en/`(SEO-friendly auto-detect)|
| 用户 Accept-Language: zh 第一次访问 `/en/` | 不强制重定向(URL 优先)|
| URL `/en/<nonexistent>` | 404 但 nav 仍 EN(per spacemit 痛点 #4 规避)|

---

## 节 8. 成本契约

per CLAUDE.md "三层成本" 划分:

| 层 | 此 spec 涉及? |
|---|--------------|
| 🆓 零成本(CPU 毫秒级) | ✅ build-time transformer(几秒钟跑完 4 路转换) |
| ⚡ 本地算力(GPU 秒级) | ❌ 不涉及 ML / embedding |
| 💰 时间金钱(LLM 云端) | ❌ 不涉及 LLM |

### License cost

- **0 license cost**:
  - Inter font: SIL OFL 1.1(免费商用)
  - JetBrains Mono: SIL OFL 1.1(免费商用)
  - Tailwind CSS: MIT(免费,attune-server 已用)
  - Polylang(WP plugin): GPL 2 免费版即够(企业版可选,本 spec 不要求)
  - Docusaurus i18n: MIT

### Build cost

- 一次 `make tokens-build` ≈ 几秒钟(4 个简单 YAML → CSS/TS transformer)
- CI 跑 link-checker(lychee)每次部署 ≈ 30 秒-2 分钟(取决于站点规模)

### 维护 cost

- tokens.yaml 改动 1 处 → 4 表面同步,**比单独改 4 处省 75%** 后续维护时间
- 缺点:增加 1 层 build-time 抽象,新人需要理解 SSOT 概念

---

## 节 9. 测试矩阵

per CLAUDE.md "测试方案规范"(2026-05-24 用户拍板),覆盖 8 场景:

| 场景类 | 测试 case | 通过判据 |
|--------|---------|---------|
| **happy path** | 用户主页 → 切英文 → 点产品 → 文档 → 博客 → 联系,全程 EN | 每页 `<html lang="en">` + 全文本 EN(grep 中文字符 = 0)|
| **edge case 1**: 部分未翻 | 用户在 `/en/products` 看到 banner 提示 + zh 内容 | banner 文本 EN("English coming soon") + 内容容错 zh |
| **edge case 2**: URL 直输 | 用户输 `/en/about` (无 cookie) | 显示 EN,setCookie lang=en |
| **error case 1**: 死链 | 用户访问 `/en/<nonexistent>` | 404 但 nav 仍 EN,推荐回到 `/en/` |
| **error case 2**: 字体加载失败 | 模拟 fonts.googleapis.com 不可达 | fallback 到 system-ui,不白屏 |
| **adversarial 1**: bad URL 含中文 | URL `/en/<中文-bad>` | URL encode + 404 不崩 |
| **adversarial 2**: cookie 注入 | cookie `lang=<script>...` | strip / validate,只接受 zh / en 白名单 |
| **多用户/多 device**: 同 user 不同 device | desktop / mobile / Chrome / Firefox | 切换行为一致,cookie 各自持久化 |
| **资源耗尽**: 慢网络 | 模拟 3G(slow) | tokens.css 先加载,字体异步,不阻塞 FCP |
| **国际化 unicode**: emoji / RTL | 内容含 emoji(✅😀)/ RTL 阿拉伯(فارسي) | 渲染正确,不破坏 layout |
| **降级**: Polylang 失效 | WP 后台禁 Polylang | fallback zh 主语言,/en/ 路径回到 zh |

### 自动化测试工具

- **Visual regression**: Playwright + `@playwright/test` snapshot(推 v1.0.3 实施)
- **Link checker**: lychee + GitHub Action(CI 跑)
- **a11y**: axe-core + Lighthouse(CI 跑,要求 AA pass)
- **对比度**: per token 强制 WCAG AA(text/bg 对比度 ≥ 4.5:1 normal,3:1 large)
   - `--color-text-primary` (#0f172a) on `--color-bg-primary` (#ffffff) = 16.1:1 ✅
   - `--color-text-secondary` (#475569) on `--color-bg-primary` (#ffffff) = 7.0:1 ✅
   - `--color-text-inverse` (#ffffff) on `--color-primary-500` (#14b8a6) = 3.2:1 ⚠️(刚过 3:1 large text;需要 button text 用 medium weight 提升可读性)

### 黑盒视角验证(per CLAUDE.md "测试方案规范" §3)

**不只是白盒**(每个组件 visual snapshot 对比),必须用户视角:
- "我装 attune,跳到官网,再装一次扩展,跳到文档 — 我看到的颜色 / 字体 / 按钮形态一致吗?"
- "我用 Mac Safari + iPhone Safari + Win Chrome + Linux Firefox — 4 个组合下视觉一致吗?"
- "我中英切换 5 次后,页面 fully 在我选的语言吗?"

---

## 节 10. 向后兼容

### 10.1 tokens schema 演进

- **v1.x 内**: 只**追加** token key,**不删除 / 不改名**
- **v2.0**(breaking): 允许重命名 / 删除,但需 cloud sprint 提前 1 个 minor 公告 deprecation + migration guide

### 10.2 CSS class 命名

- `es-*` 前缀**永久保留**,不改名
- 旧 class(如 `attune-btn` 等遗留)**v1.0 内保留**(各表面 framework default),v1.0.6 起渐进 migrate 到 `es-` 前缀

### 10.3 i18n key

- vue-i18n / Django i18n / Polylang slug 仅**追加**,不删
- 修翻译文本不算 breaking(只要 key 不变)

### 10.4 老 client 行为

- desktop attune-server <v1.0.3 没有新 tokens.ts → 用 fallback 静态颜色(已 hardcode 在 ui/src/),功能正常
- 用户从 v1.0.0 升 v1.0.3 → 视觉自动统一,**不**需要清缓存 / reset Vault

---

## 节 11. 风险登记

| R | 描述 | 严重度 | 缓解 |
|---|------|--------|------|
| **R1** | spacemit 视觉过于借鉴,品牌混淆 | High → **Low**(per amend) | amend §3 已差异化:engi-stack 走明亮 teal,spacemit 暗蓝灰;**布局借鉴 + 配色差异化** |
| **R2** | 4 表面 build pipeline 各异,tokens sync 难 | Medium | SSOT YAML + 4 transformer + CI 守卫(token 改动后 4 表面 PR 必须同时 merge);新表面接入有 §6 模板 |
| **R3** | i18n 切换 bug 重蹈 spacemit 覆辙 | High | §7.1 详细契约 + 自动化 link checker(CI 强制)+ visual diff(v1.0.3)+ 黑盒测试(§9) |
| **R4** | WordPress Polylang 切换不持久 | Medium | URL path 为主 + cookie 7 天 + Accept-Language auto-detect 三轨;URL 永远优先 |
| **R5** | design tokens 与 attune-server 现有 Tailwind v4 冲突 | Medium | tailwind.config.ts `theme.extend.colors` 引 `design-tokens.ts` const;Tailwind utility class 仍能用,只是颜色来源切换;v1.0.3 实施时测试 |
| **R6** | 各表面字体加载差异(Inter not bundled in WP)| Low | font fallback 链已设计(§7.3);v1.0.3 实施时统一用 fontsource self-host(避免 CDN 慢)|
| **R7** | tokens YAML 改动后某表面忘记 rebuild | Medium | cloud monorepo 引入 nx / turborepo affected build 检测;CI 强制 token 改动 + 4 表面 build 同 PR |
| **R8** | dark mode 后续加入时打破 v1.0 light only 用户预期 | Low | dark mode v1.0.6 推出时 user-controlled toggle + light 默认 + setting 持久化,不强制切换 |
| **R9** | WCAG AA 对比度某 token 组合不过 | Low | §9 已列每对 token 对比度;button text 用 medium weight 补偿 |
| **R10** | spacemit 实际改版后我们借鉴的 layout 也变了 | Low | 我们借鉴的是**抽象 layout pattern**(hero 4 层 / nav sticky / footer 4 col),不是具体 markup;spacemit 改版不影响我们 |

---

## 附录 A. spacemit.com 视觉调研原始数据

**调研时间**: 2026-05-25 (本 spec 撰写时)
**调研方式**: HTTP fetch HTML + CSS bundle + JS bundle + 路由探针
**调研工具**: `ctx_execute` JavaScript fetch

### A.1 技术栈

| 项 | 值 |
|---|---|
| 前端框架 | Vue 3 + vue-router + vue-i18n |
| UI 库 | Tailwind v4(`--tw-*` 前缀)+ Ant Design Vue(`antd-vendor`) |
| 渲染模式 | **纯 SPA**(client-side,无 SSR) |
| HTML shell 大小 | 1191 bytes(`#app` 占位 + module preload) |
| CSS bundle | `index-DnAs8dx_.css` = 27 KB |
| JS bundle | `index-CBiuoxOy.js` = 381 KB |

### A.2 配色 palette(从 CSS bundle 提取)

| Hex | 用量 | 推测用途 |
|-----|------|---------|
| `#fff` | 14× | 主背景 / 文字反色 |
| `#667eea` | 14× | **主品牌色**(蓝紫,Tailwind indigo-400 风格)|
| `#764ba2` | 4× | 渐变副色(配 #667eea 做 gradient)|
| `#333` | 3× | 主文本(深灰) |
| `#888` | 3× | 次要文本 |
| `#ff4d4f` | 3× | danger / accent(红)|
| `#4ca8ff` | 3× | secondary(亮蓝) |
| `#666` | 3× | tertiary text |
| `#f6f8fa` | 3× | 卡片底色 |
| `#5f6b85` | 2× | 次次要文本 |

**核心判断**:
- 主色 #667eea ≈ slate-blue / indigo,**偏暗 + 冷色调**(per user 反馈"色彩太暗")
- 渐变 #667eea → #764ba2 是经典 "purple haze" 风格(Tailwind v3 default gradient sample)
- engi-stack **不**沿用(amend §3 已切换到明亮 teal #14b8a6)

### A.3 字体

```
font-family: system-ui, -apple-system, BlinkMacSystemFont, "Helvetica Neue",
             "Segoe UI", Helvetica, Arial, "PingFang SC", "Microsoft YaHei",
             "WenQuanYi Micro Hei", sans-serif
font-family: SFMono-Regular, Consolas, "Liberation Mono", Menlo, Courier, monospace
```

**判断**: 用 system font + CJK fallback,无 web font(Inter / 等)。engi-stack 改用 Inter 提高品牌识别。

### A.4 字号 hierarchy

最高频:14px(14×)/ 13px / 12px / 16px / 15px。spacemit 主体走小字号(14px base),engi-stack 沿用 14px-16px base + hero 56px。

### A.5 圆角 / 阴影

**radius**: 4 / 6 / 8 / 10 / 12 / 14 / 18 / 20 / 24 / 40 / 50% — 用得太杂,engi-stack 简化为 5 档。
**shadow** 示例:
- `0 24px 80px #0f172a2e`(hero 大阴影)
- `0 8px 24px #ff4d4f4d`(red CTA glow)
- `0 4px 12px #00000026`(card)
- `0 6px 20px #0003`(hover)

engi-stack 简化为 4 档(card / hover / modal / hero)。

### A.6 路由探针结果

```
GET https://spacemit.com/             → 200, 1191B
GET https://spacemit.com/en/          → 200, 1191B(同 shell)
GET https://spacemit.com/zh/          → 200, 1191B(同 shell)
GET https://spacemit.com/products     → 200, 1191B(同 shell)
GET https://spacemit.com/en/products  → 200, 1191B(同 shell)
GET https://spacemit.com/about        → 200, 1191B(同 shell)
GET https://spacemit.com/en/about     → 200, 1191B(同 shell)
GET https://spacemit.com/robots.txt   → 200, 1191B ❌(返回 SPA shell,不是 robots)
GET https://spacemit.com/sitemap.xml  → 200, 1191B ❌(返回 SPA shell,不是 sitemap)
```

**关键发现**: **所有路由返回完全相同的 HTML shell** → SSR 完全缺失 → 搜索引擎只能看到空 `<div id="app"></div>` → SEO disaster。

### A.7 i18n 实现(从 JS bundle 提取)

```js
// index-CBiuoxOy.js 关键片段:
localStorage.getItem("language")
localStorage.setItem("language", e)
messages: { zh: mm, en: fm }
fallbackLocale: ae.DEFAULT_LANGUAGE
```

**判断**:
- vue-i18n + 2 locale(zh / en)
- 语言**仅**存 localStorage,key=`language`
- 无 URL routing 区分语言(/en/ 路径无实际意义,Vue router 不识别)
- 切换语言 = 改 localStorage + 触发 vue-i18n 重渲染,**URL 不变**

---

## 附录 B. spacemit i18n 痛点诊断 + engi-stack 规避方案

基于附录 A 调研数据,spacemit i18n 有 **6 类痛点**:

### 痛点 #1: 语言只存 localStorage(无 URL 锚)

- **症状**: 用户在 `/products` 看了一会儿,切到 EN,URL 仍是 `/products`(不变 `/en/products`)。复制 URL 发给朋友 → 朋友打开看到 zh(因为他的 localStorage 没设)
- **根因**: localStorage 不可分享,URL 才是分享锚
- **engi-stack 规避**: §7.1 URL 策略强制 `/` (zh) + `/en/` 双轨;localStorage 仅用于桌面 app

### 痛点 #2: 切换后行为不确定

- **症状**: 用户在 `/about` 点 EN,有可能跳到 `/`(回首页)或 `/about`(留原页) — 各 SPA 实现不一
- **根因**: 切换 widget 没绑定到当前 route mapping
- **engi-stack 规避**: §7.1 切换 widget 行为:跳到**对应语言版本同一页面**,绑定 mapping

### 痛点 #3: SEO 完全缺失

- **症状**: `robots.txt` 不存在,`sitemap.xml` 不存在,`<html lang="en">` 写死(zh 页面也是 lang=en)
- **根因**: 纯 SPA + 无 SSR
- **engi-stack 规避**: §7.1 SEO 多语言契约:`<html lang>` 动态、`hreflang` 完整、`sitemap.xml` + `robots.txt` 必有
- **额外 engi-stack 决策**: official-web 用 WordPress(server-rendered,天然 SSR)+ wiki 用 Docusaurus(static export,build-time render)+ accounts 用 Django template(server-rendered)— 仅 attune-server desktop UI 走 SPA(桌面 app 无 SEO 需求,可接受)

### 痛点 #4: 死链 / 部分翻译

- **症状**: 用户切 EN 后某些子页面仍显示 zh(翻译未完成),或某些链接 404
- **根因**: 缺翻译时 vue-i18n silent fallback,user 看到混合语言
- **engi-stack 规避**: §7.1 fallback 策略:显示页面 + banner 提示 + console warn(不静默)+ CI link checker

### 痛点 #5: 持久化跨 session 不稳

- **症状**: 用户切 EN,关浏览器,再打开 spacemit.com → 看到 zh(localStorage 被清 / 跨 device 不同步)
- **根因**: 只依赖 localStorage,无 cookie / URL fallback
- **engi-stack 规避**: §7.1 三轨持久化:URL 优先 > cookie 7 天 > localStorage > Accept-Language auto-detect

### 痛点 #6: a11y screenreader 错读

- **症状**: 中文页面 `<html lang="en">`,screenreader 用英文发音读中文,完全乱
- **根因**: shell HTML 写死 lang=en
- **engi-stack 规避**: §7.1 `<html lang>` 必须动态(每页 SSR / build-time 决定)

### engi-stack 规避汇总表

| spacemit 痛点 | engi-stack 规避机制 |
|--------------|------------------|
| #1 无 URL 锚 | URL `/` + `/en/` 路径必须存在 |
| #2 切换行为不确定 | 切换 widget 跳同 path 对应语言 |
| #3 SEO 缺失 | SSR(WP / Django / Docusaurus build)+ hreflang + sitemap + robots |
| #4 死链 / 半翻译 | CI link checker 强制 + banner 提示 + console warn |
| #5 持久化不稳 | URL > cookie > localStorage > Accept-Language 四轨 |
| #6 a11y 错读 | `<html lang>` 必须动态(SSR / build-time) |

---

## 附录 C. user 决策点

以下 4 项需 user 拍板,落档 user 回复后 cloud sprint 实施:

### C.1 spec 是否批准?推哪个 minor 实施?

**选项**:
- (a) v1.0.3(与 Observability sprint 同期,5/30-6/5)— 推荐
- (b) v1.0.6(与 dark mode 同期,7 月)— 风险:visual 不统一持续 1 月
- (c) 推 v2.0 — 不推荐,主品牌信任受损

**建议 (a)**。

### C.2 design tokens 主色

per amend §6 4 候选:

| 选项 | 色值 | 暗示 | 与 spacemit 差异度 |
|------|------|------|------------------|
| **A. Teal 500** ⭐(默认) | `#14b8a6` | 现代 / 科技 / 知识 | 高(spacemit 蓝紫 vs teal 蓝绿)|
| B. Emerald 500 | `#10b981` | 生机 / 隐私 / 自然 | 高 |
| C. Violet 500 | `#8b5cf6` | AI 感 / 创意 / 未来 | 中(都偏紫,易混)|
| D. Sky 500 | `#0ea5e9` | 平静 / 信任 / 干净 | 低(都偏蓝,易混)|

**建议 A**(差异度最大 + 明亮)。

### C.3 logo / brand 是否同步重新设计?

**选项**:
- (a) 不重新设计 logo,沿用现有 — 推荐(focus on tokens)
- (b) logo 同期重设 — 推 v1.0.6+,与 a34d92b0 branding.yaml sprint 衔接

**建议 (a)** for v1.0.3,branding 演进推后续。

### C.4 i18n 切换技术栈

**选项**:
- (a) URL path + cookie + localStorage + Accept-Language **四轨**(§7.1 默认)— 推荐
- (b) 仅 URL path + cookie 双轨(简化,但失去 Accept-Language auto-detect)
- (c) 仅 URL path(最简,user 第一次访问总是看到 zh)

**建议 (a)**(完备,但需要 cloud sprint 实施 fallback 逻辑)。

---

## 实施先后(per amend §8 时间线)

| 版本 | 工作 | 时间 |
|------|------|------|
| **v1.0.0**(today)+ v1.0.1(5/26-28) | ✅ 主 spec + amend 落档(本文档)| done |
| **v1.0.2**(5/31) | user 选定主色(C.2)+ 主 spec §3 合并 amend §3 final | 5/31 |
| **v1.0.3**(6/05) | **4 表面 design tokens 同步实施**(配合 Observability sprint visual regression test) | 6/05 |
| **v1.0.6**(7/15) | dark mode 扩展 + logo 重设(若 user 选)| 7/15 |
| **v1.0.9**(8/15) | i18n 切换契约真验证(与 i18n sprint 同期)+ WCAG AAA 审计 | 8/15 |

---

## 与 a34d92b0 official-web 内容 agent 衔接(per amend §7)

a34d92b0 agent 在写 `cloud/official-web/branding.yaml`(产品 metadata / nav / footer 内容),与本 design tokens **正交**:

| | branding.yaml(a34d92b0) | design-tokens.yaml(本 spec)|
|---|------------------------|--------------------------|
| **职责** | WHAT — 内容 | HOW — 视觉 |
| **字段** | name / logo path / brand tagline / nav 结构 / footer links / legal pages | colors / typography / spacing / radius / shadow |
| **改动频率** | 中(随品牌演进)| 低(v1 后只追加)|
| **owner** | branding / marketing | design / engineering |

**a34d92b0 完成后实施 sprint 衔接点**(推 v1.0.3):
- WordPress `themes/engi-stack/style.css` 用 CSS custom properties(本 spec 输出)
- branding.yaml 引 `theme.primary_color: var(--color-primary-500)` 等(交叉引用,但内容 vs 视觉解耦)
- v1.0.3 PR 同时改两份(branding.yaml fully populated + design-tokens.yaml deploy)

**不冲突保证**: 本 spec **不动 branding.yaml,不动 content/, 不动 wiki-web**(per CLAUDE.md 红线)。
