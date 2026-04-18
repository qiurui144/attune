# Attune UX Quality — a11y · i18n · 帮助系统 · 空状态

**Date:** 2026-04-19
**Status:** 待实施
**Scope:** Attune 前端（rust/crates/attune-server/ui）UX 品质基础设施
**Parallel to:** `2026-04-19-frontend-redesign-design.md`（UI 重构主 spec）
**Part of:** "产品级基础框架" 系列 3 个并行 spec 中的 2b

---

## 0. 背景与动机

### 问题

UI 重构 spec 定好了视觉 + 交互 + 架构，但以下 5 个 UX 品质维度**必须在组件第一次写出来时就内置**，否则后续补代价极高：

| 维度 | 不做会怎样 |
|------|-----------|
| **键盘可访问** | 键盘效率用户弃用；意外也违反国际 a11y 基线 |
| **基础 a11y** | 视障用户完全不能用；不符合企业采购合规（很多律所要求） |
| **i18n 框架** | 所有中文硬编码，国际版要全文件重写 |
| **快捷键注册 + 帮助** | 用户不知道 `⌘K` 能搜索，产品"深度"藏而不露 |
| **空状态教育** | 新用户第一次看到空 Chat 面板不知道能做什么 |

### 目标

- 在 UI 重构 spec 的 Preact 组件实施时**同步**构建这 5 层
- 不追求 WCAG AAA 全量（YAGNI），做 AA 基线 + 实用键盘快捷键
- i18n 用轻量自写框架（不引 `@lingui` `i18next` 等重库，保持 bundle 小）
- 所有 copy 通过 `t()` 函数；开发时 lint 禁止硬编码中文

### 非目标

- RTL 布局（阿拉伯语等） defer
- 语音控制 / 语音输入 defer
- 屏幕阅读器的完整叙事体验（我们做到 landmarks + labels，不做 live regions 等高级项）
- 方言（繁中 / 粤语 / 等） defer

---

## 1. 键盘导航基线

### 设计原则

每个 interactive 元素必须：
1. **可 Tab 到达**（tabindex="0" 默认；跳过纯展示元素用 tabindex="-1"）
2. **焦点可见**（统一 focus ring：`2px solid #5E8B8B` + `2px offset`）
3. **Enter/Space 激活**（按钮用 `<button>` 天然支持，自定义组件手动绑 onKeyDown）
4. **Esc 取消**（modal / drawer / popover 统一 Esc 关闭）

### Tab order 约定

- DOM 顺序 == 视觉顺序，避免 `tabindex > 0` 这种手动打乱
- Sidebar → Main → Drawer（如打开），按空间顺序
- Modal / drawer 打开时**焦点陷阱**（focus trap）：Tab 循环在 modal 内部不跳出

### 焦点管理（Focus Management）

| 事件 | 焦点应到哪 |
|------|-----------|
| 页面加载 | 第一个 interactive 元素（通常是 `+新对话`） |
| Modal 打开 | Modal 内第一个 interactive（通常关闭按钮或首个输入） |
| Modal 关闭 | 回到触发打开的元素 |
| Drawer 打开 | Drawer 内容区首个 interactive |
| View 切换 | Main 区顶部 heading（通过 programmatic focus） |

实现：`useFocusTrap()` hook + `lastFocusedRef` ref 记录触发元素。

### 组件 baseline contract

所有 UI 组件必须满足：

```tsx
// Button.tsx — 契约示例
interface ButtonProps {
  // A11y required
  'aria-label'?: string;      // 无文字内容时必填
  'aria-pressed'?: boolean;   // toggle 按钮
  disabled?: boolean;         // 设 true 时 aria-disabled 也 true

  // Keyboard required（天然由 <button> 支持）
  onClick: () => void;

  // Visual
  variant: 'primary' | 'secondary' | 'ghost' | 'danger';
  size: 'sm' | 'md' | 'lg';
  children: ComponentChildren;
}
```

### 验收清单（WCAG 2.1 AA）

- [ ] 所有交互元素可键盘到达
- [ ] 焦点环可见（not outline: none 裸删）
- [ ] 颜色对比度 ≥ 4.5:1 正文，≥ 3:1 大字号
- [ ] 表单 input 有 `<label>` 或 `aria-labelledby`
- [ ] Button 无文字时有 `aria-label`
- [ ] Modal 有 `role="dialog" aria-modal="true" aria-labelledby`
- [ ] Image 有 `alt`（装饰性 alt=""）
- [ ] Landmarks 清晰（`<nav>` `<main>` `<aside>` `<footer>`）
- [ ] 错误消息关联到字段（`aria-describedby`）

不做（超出 AA 范围）：
- Skip links（小应用用处不大）
- Live regions（Chat 流式输出用普通 DOM 更新已足够）
- Offline accessibility tree cache

---

## 2. i18n 基础设施

### 引擎选择

**自写轻量 i18n**，不引第三方库。理由：

- bundle 预算紧（总 ~80KB），@lingui 就 ~10KB
- 需求简单：key 查表 + 参数插值 + 极少数 plural form
- locale 文件是 plain JSON，工具链无需复杂

### 核心 API

```ts
// i18n/core.ts (约 80 行，无依赖)

type Locale = 'zh' | 'en';
type Messages = Record<string, string | { one: string; other: string }>;

const MESSAGES: Record<Locale, Messages> = { zh, en };
let currentLocale: Locale = 'zh';

export function setLocale(locale: Locale) {
  currentLocale = locale;
  // 触发全局 re-render
  localeSignal.value = locale;
}

export function t(key: string, params?: Record<string, string | number>): string {
  const msg = MESSAGES[currentLocale]?.[key] ?? MESSAGES.zh?.[key] ?? key;
  const text = typeof msg === 'string' ? msg : msg.other;
  return params ? interpolate(text, params) : text;
}

export function plural(key: string, count: number, params?: Record<string, string | number>): string {
  const msg = MESSAGES[currentLocale]?.[key] ?? MESSAGES.zh?.[key];
  if (typeof msg === 'object') {
    const text = count === 1 ? msg.one : msg.other;
    return interpolate(text, { ...params, count });
  }
  return t(key, { ...params, count });
}

function interpolate(text: string, params: Record<string, any>): string {
  return text.replace(/\{(\w+)\}/g, (_, k) => String(params[k] ?? `{${k}}`));
}
```

### Locale 文件结构

```
ui/src/i18n/
├── core.ts              # 引擎
├── zh.ts                # 中文消息（主源）
├── en.ts                # 英文消息
└── keys.ts              # TypeScript 类型化 key 列表（autocomplete + compile-time 检查）
```

```ts
// zh.ts 示例
export const zh = {
  // Wizard
  'wizard.welcome.title': 'Attune · 私有 AI 知识伙伴',
  'wizard.welcome.sub': '本地决定，全网增强，越用越懂你的专业',
  'wizard.pwd.heading': '设置 Master Password',
  'wizard.pwd.warning': '忘记无法找回',

  // Chat
  'chat.empty.title': '问点什么吧',
  'chat.empty.sample1': '帮我检索关于专利 IPC A61K 的卷宗',
  'chat.input.placeholder': '输入问题...',
  'chat.send': '发送',

  // Common
  'common.save': '保存',
  'common.cancel': '取消',
  'common.delete': '删除',
  'common.retry': '重试',

  // Plural
  'items.count': { one: '{count} 条', other: '{count} 条' },  // 中文无单复数，但保持结构

  // With params
  'error.network': '网络错误：{message}',
  'hint.local_empty_no_browser': '本地知识库无相关内容；网络搜索不可用（未检测到 Chrome 或 Edge）',
} as const;

export type MessageKey = keyof typeof zh;
```

### 使用

```tsx
// 组件中
import { t, plural } from '@/i18n';

<h1>{t('wizard.welcome.title')}</h1>
<p>{t('error.network', { message: err.message })}</p>
<span>{plural('items.count', items.length)}</span>
```

### 语言选择策略

1. **首次启动**：读 `navigator.language` → 匹配到支持的 locale；否则默认 `zh`
2. **用户偏好**：Settings > 语言，写入 `app_settings.language`
3. **启动时**：先读 `app_settings.language`，无则 fallback 到步骤 1

### 提取 + 静态检查工具链

**阶段 1（手动维护）**：
- 每加一行用户可见文字，手动加 key 到 `zh.ts` 和 `en.ts`
- TypeScript 的 `MessageKey` 类型保证 `t(key)` 编译期检查 key 存在

**阶段 2（lint rule）**：
- ESLint rule 禁止 JSX 里的裸中文字符串（`\p{Unified_Ideograph}` 正则 detect）
- 例外：代码注释、`data-testid`、`aria-hidden="true"` 元素的文字
- PR 自动 CI fail 如有违规

阶段 2 规则文件：

```js
// .eslintrc.cjs
module.exports = {
  rules: {
    'no-hardcoded-chinese': {
      create(context) {
        return {
          JSXText(node) {
            if (/[\u4e00-\u9fff]/.test(node.value) && !node.value.trim().startsWith('//')) {
              context.report(node, 'Hardcoded Chinese text; use t() instead');
            }
          },
          Literal(node) {
            // JSX attribute string literals with Chinese
            if (typeof node.value === 'string' && /[\u4e00-\u9fff]/.test(node.value)) {
              if (node.parent?.type === 'JSXAttribute') {
                const attr = node.parent.name?.name;
                if (['aria-label', 'title', 'placeholder', 'alt'].includes(attr)) {
                  context.report(node, `Chinese in ${attr}; use t() instead`);
                }
              }
            }
          },
        };
      },
    },
  },
};
```

### English 翻译策略

- **不机器翻译**（机翻质量烂）
- 初版 `en.ts` 由人工写（先英文开发者，再母语者 review）
- 发布前关键路径（wizard + 错误消息 + 主 CTA）100% 覆盖
- 非关键（help docs、changelog 等） Chinese fallback OK（`t()` 自动 fallback zh）

### Bundle size

- 引擎 core.ts：~1KB
- zh.ts 全量消息：预计 5-8KB
- en.ts 全量消息：预计 5-8KB
- **总 overhead：~15KB gzip 前、5KB gzip 后**

---

## 3. 键盘快捷键注册 + 帮助覆盖层

### 全局快捷键表

| 快捷键 | 效果 | 生效位置 |
|-------|------|---------|
| `⌘K / Ctrl+K` | 全局搜索（items + conversations） | 任何视图 |
| `⌘N / Ctrl+N` | 新对话 | 任何视图 |
| `⌘, / Ctrl+,` | Settings view | 任何视图 |
| `⌘/ / Ctrl+/` | 呼出快捷键列表 overlay | 任何视图 |
| `⌘↵ / Ctrl+Enter` | 发送 chat 消息 | Chat input 聚焦时 |
| `Esc` | 关闭 modal / drawer / overlay | 有激活浮层时 |
| `?` | Context help drawer（当前视图） | 任何视图 |
| `⌘B / Ctrl+B` | 切换 sidebar 折叠 | 任何视图 |
| `⌘⇧D` | 切换 dark/light 主题 | 任何视图 |
| `⌘L / Ctrl+L` | 锁定 vault | 任何视图（已 unlocked 时） |
| `↑ / ↓` | Chat 输入历史回溯 | Chat input 聚焦时 |

**不做**（太小众 / 容易冲突）：
- Vim 模式（j/k 导航）
- 自定义键位（Tier C 层，后续）

### 注册机制

```ts
// hooks/useShortcut.ts
type Shortcut = {
  key: string;          // e.g. 'k'
  meta?: boolean;       // Cmd/Ctrl
  shift?: boolean;
  alt?: boolean;
  when?: () => boolean; // 条件（如 "chat input 聚焦时"）
  handler: (e: KeyboardEvent) => void;
  description: string;  // i18n key for help overlay
};

export function useShortcut(shortcut: Shortcut) { /* ... */ }
export const registeredShortcuts = signal<Shortcut[]>([]);
```

组件注册：

```tsx
function ChatView() {
  useShortcut({
    key: 'Enter',
    meta: true,
    when: () => isChatInputFocused(),
    handler: () => sendMessage(),
    description: 'shortcut.chat.send',
  });
}
```

### Help overlay（`⌘/` / `?` 呼出）

- 半透明背景遮罩（点击外部关闭）
- 居中卡片列出当前生效的所有 shortcuts
- 按来源分组：全局 / 当前视图
- 每条显示：按键组合 + 说明（从 i18n key 取）

### Context help drawer（`?` 键）

- 右侧 slide-in drawer（共用 §4 UI spec 的 drawer 组件）
- 内容：针对当前视图的 Markdown 文档
- 内容文件：`ui/src/help/{view}.md`（chat.md / items.md / remote.md / knowledge.md / settings.md）
- 渲染：`preact-markdown` 或自写简易 markdown parser（~2KB）

---

## 4. In-app 帮助系统

### 文档结构

```
ui/src/help/
├── index.md           # 欢迎页，链接到各视图
├── chat.md            # Chat 使用说明
├── items.md           # 条目浏览、批注
├── remote.md          # WebDAV 绑定
├── knowledge.md       # 聚类与知识全景
├── settings.md        # Settings 各项说明
├── plugins.md         # 插件安装、PluginHub
└── troubleshooting.md # 常见问题
```

### 文档模板

每个 `.md` 文件格式：

```markdown
# [视图名] 使用说明

## 快速入门
[2-3 段，核心功能]

## 快捷键
- `⌘K`：搜索
- ...

## 进阶用法
### 场景 1
...

## 常见问题
- Q: ...
  A: ...
```

### 构建时处理

- Markdown 文件通过 `import help from './help/chat.md?raw'` 导入
- Vite 自带 `?raw` 支持，无需插件
- 运行时用轻量 markdown renderer（~2KB）渲染到 drawer
- 代码块、链接、标题都支持；表格、公式 defer

### i18n 处理

阶段 1：help 文档只提供中文版
阶段 2：每文件加 en 兄弟文件 `chat.en.md`，按 locale 选择

### 触发入口

- Sidebar 底部账户菜单：`帮助中心` → 打开 help index
- 每个主视图顶栏：`?` 图标按钮 → 打开对应视图 help
- 键盘：`?` → context help

---

## 5. 空状态教育内容

### 模式

统一组件 `<EmptyState>`：

```tsx
interface EmptyStateProps {
  icon: ComponentChildren;          // 大图标或插画
  titleKey: string;                 // i18n key
  descriptionKey: string;
  actions?: Array<{
    labelKey: string;
    onClick: () => void;
    variant: 'primary' | 'ghost';
  }>;
  examples?: string[];              // 示例 prompts/查询（chat 用）
}
```

### 内容清单

| 空状态 | Icon | 标题 | 描述 | 操作 |
|-------|------|------|------|------|
| 空 Chat（新对话） | 💬 | `chat.empty.title` "问点什么吧" | 基于你的知识库，或搜索全网 | 4 个 sample prompts chips |
| 空条目列表 | 📄 | "还没有录入内容" | 拖拽文件或绑定文件夹 | "上传文件" / "绑定文件夹" 按钮 |
| 空远程目录 | 🔗 | "还没绑定任何 WebDAV" | 连接 Nextcloud / 自建 WebDAV | "添加目录" |
| 空知识全景 | 📊 | "还没发现聚类" | 需要至少 20 条记录才能聚类 | "上传文件" |
| 空搜索结果 | 🔎 | "没找到匹配的内容" | 本地无此内容，可以问 Chat 从网络找 | "用 Chat 问" |
| 空会话列表 | 📝 | "开始第一次对话" | — | "新对话" |

### Chat 空状态的 sample prompts

根据检测到的领域插件动态显示。默认通用 prompts：

- `帮我总结一下最近上传的文件`
- `搜索关于 XXX 的所有内容`
- `我上次讨论了什么话题？`

如果 `law` 插件开启：
- `这个合同条款有什么风险？`
- `帮我找类似案件的判决书`

如果 `patent` 插件开启：
- `这个技术方案在 USPTO 有先行技术吗？`

### 零售 vs 教育平衡

- 空状态不卖力"教课"（避免像 PowerPoint 过场）
- 一句话 title + 一句话 description + 1-2 个明确 CTA 即可
- Sample prompts 作为 chip，点了直接填充输入框，低摩擦

---

## 6. 与 UI 重构 spec 的协同

### 强制协同点

UI 重构 spec 实施时必须遵守：

| UI spec 组件 | 本 spec 要求 |
|-------------|-------------|
| 所有 Button / Input / Select | 必须走 §1 的 baseline contract |
| 所有用户可见文本 | 必须 `t(key)` 而非硬编码中文 |
| Modal / Drawer / Popover | 必须 focus trap + Esc 关闭 |
| 空视图（Chat / Items / Remote / Knowledge）| 必须渲染 `<EmptyState>` 而非 blank |
| 视图顶栏 | 必须有 `?` 图标触发 context help |
| 全局 app shell | 必须挂载 `GlobalShortcutListener`（监听 ⌘K / ⌘/ / ⌘N 等） |

### 实施顺序

**不独立新分支**，跟 UI 重构 spec 同一分支、同一 PR 推进。开发顺序：

1. 先建 `i18n/core.ts` + `zh.ts` 骨架（~1 天）
2. 先建 `useShortcut` hook + `useFocusTrap` hook（~1 天）
3. 先建 `<EmptyState>` + `<Button>` + `<Input>` 基础组件（带 a11y contract）（~2 天）
4. 以上就位后，UI 重构 spec 的组件用这些 primitive 一路建
5. Help drawer + help markdown 最后补（~2 天）

---

## 7. 测试策略

### a11y 自动化

- `@axe-core/playwright` 跑每个主视图 axe 扫描
- CI fail if critical/serious 等级 violation > 0
- moderate 等级警告，不 fail

### 键盘可达性测试

Playwright 脚本：
```ts
test('Chat view keyboard nav', async ({ page }) => {
  await page.goto('/');
  await page.keyboard.press('Tab'); // 到 sidebar 首个元素
  await page.keyboard.press('Tab'); // 继续...
  // 验证 focused element 序列符合预期
});
```

### i18n 覆盖率测试

脚本扫描所有 `.tsx` 文件，确认：
- 无裸中文（ESLint rule）
- 所有 `t(key)` 的 key 在 `zh.ts` 和 `en.ts` 都有条目
- `en.ts` 关键路径 key 覆盖率 ≥ 80%（wizard / error / 主 CTA）

### 空状态 snapshot

每个空状态组件 snapshot 测试，防止 copy 被意外改动。

---

## 8. 成功标准

### 功能验收

- [ ] 键盘从 sidebar 到 drawer 全链路可达（无需鼠标）
- [ ] `⌘K / Ctrl+K` 全局搜索 · `⌘/` 快捷键列表 · `?` 帮助 drawer 生效
- [ ] 切换 locale (zh → en) 全界面文字即时更新无刷新
- [ ] axe-core 扫描 critical 级违规 = 0
- [ ] 每个主视图的空状态都有示例 prompts/actions
- [ ] ESLint 禁止裸中文 rule 生效 + CI 拦 PR

### 性能指标

- i18n 引擎 + 消息包 overhead ≤ 15KB gzip 前
- keyboard shortcut 注册 ≤ 1ms/个
- help drawer 首次打开 ≤ 200ms

### 覆盖率指标

- 全部可见文字走 `t()`（100%，lint 保证）
- en.ts 关键路径覆盖 ≥ 80%
- 所有主视图有空状态（100%）

---

## 9. 范围外（单独 spec 推进）

- RTL 布局 · 语音控制 · 完整屏幕阅读器优化 → 未来
- 自定义键位 → Tier C
- WCAG AAA 级（更高色对比、更严叙事）→ 未来
- 本地化其他语言（繁中、日语、德语、法语） → 依市场需求决定

---

## 10. 开放问题

1. **help markdown 是否要加图片/截图**？
   - 加：更易懂，但 bundle 涨（图片 base64 inline）
   - 不加：保持 bundle 小，靠文字 + 动图 gif 或外部 URL
   - **决策**：不加 inline 图片；复杂场景用文字描述 + 外链到 docs.attune.ai

2. **是否需要"新手教程" guided tour**（像 Notion 那种圈圈指向 UI 元素）？
   - 本 spec 不做（复杂度高）
   - Wizard 完成后进主应用的 3 秒 tip 轮播（UI spec 已规划）已足够引导
   - 后续 Tier C 可做

3. **英文翻译的质量标准**？
   - 关键路径（wizard / error / CTA）人工 review
   - 非关键（help docs）允许 LLM 辅助后 spot check
   - 绝不机翻直接上
