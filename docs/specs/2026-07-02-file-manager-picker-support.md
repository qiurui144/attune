# File Manager Picker Support — Unify All Document Path Selection

> 目标：所有文档路径的输入点，在桌面环境（Tauri）下均支持原生文件管理器弹出选择，
> 在浏览器/local scheduler Web 环境下提供浏览器所能给予的最佳回退选择体验。

## 1. 目标定位

- 解决用户痛点：当前 attune 中部分路径输入是纯文本输入框（转写路径、整理文件夹），
  部分文件选择只用浏览器原生 `<input type="file">` 而未在桌面环境下升级到原生 dialog，
  用户体验不统一。
- 与产品定位对齐：个人通用知识库，桌面优先（Windows P0 → Linux x86_64 P1），
  但同时兼顾 本地调度器设备纯 Web 与浏览器调试模式。

## 2. 范围边界

**做：**
- 抽共享 `useFilePicker` hook，统一 Tauri desktop 检测 + 原生 dialog 导入 + 浏览器回退
- 迁移 3 处已有的重复内联 dialog 调用到 hook（Wizard Step5 / Settings 文件夹管理 / RemoteView 本地绑定）
- 给 6 处缺 picker 的入口补上文件/目录选择器按钮/增强（OfficeView OCR+转写、ItemsView 上传、
  Settings 记忆导入、OrganizeWizard 路径、Wizard Step5 profile 导入）
- 浏览器回退：目录 → `webkitdirectory` + 隐藏 `<input type="file">`；文件 → 隐藏 `<input type="file">`
- 新增 3 个 i18n key（picker.browse_file / picker.browse_folder / toast.browser_no_directory）

**不做：**
- 不改变后端 API 契约（/upload、/index/bind、/profile/import 等保持不变）
- 不改 DocIntel 布局（属于独立 spec）
- 不全面美化 UI（属于独立 spec）
- 不新增 URL/远程源/凭据类输入点的 picker（WebDAV URL、Git URL、邮箱配置等 —— 它们不是文档/文件路径）

## 3. 架构数据流

```
                    ┌─────────────────────────────────┐
                    │         useFilePicker()          │
                    │                                 │
         ~~~~~~~~~~│  isDesktop     (Signal<bool>)   │~~~~~~~~~~
         │          │  picking       (Signal<bool>)   │          │
         │          │  error         (Signal<string>)  │          │
         │          │                                 │          │
    pickDirectory() │  Desktop:                       │ pickFiles()
         │          │    @tauri-apps/plugin-dialog    │          │
         │          │    open({directory:true})        │          │
         ▼          │  Browser:                       │          ▼
         │          │    <input webkitdirectory>       │          │
         │          │    → paths[]                    │          │
         │          └─────────────────────────────────┘          │
         ▼                                                       ▼
  string[] 路径数组                    { paths:string[], files:File[] }
         │                                                       │
         ├─ Step5Data bind dir                                  ├─ Step5Data import
         ├─ Settings folder mgmt                               ├─ ItemsView upload
         ├─ RemoteView local bind                              ├─ OfficeView OCR
         ├─ OrganizeWizard path                                ├─ Settings memory import
         │                                                       │
         ▼                                                       ▼
   POST /api/v1/index/bind                               POST /api/v1/upload
                                                        POST /api/v1/profile/import
                                                        POST /api/v1/memory/import
```

## 4. 模块边界

| 模块 | 文件 | 变更类型 |
|------|------|---------|
| **共享 hook** | `ui/src/hooks/useFilePicker.ts` | **新增** |
| **消费端(迁移)** | `ui/src/wizard/Step5Data.tsx` | 修改 — 删内联 canPickFolder+pickFolder |
|  | `ui/src/views/SettingsView.tsx` | 修改 — 删 FolderLinksSection 内联 |
|  | `ui/src/views/RemoteView.tsx` | 修改 — 删 LocalForm 内联 browse |
| **消费端(增强)** | `ui/src/views/ItemsView.tsx` | 修改 — 上传按钮加桌面增强 |
|  | `ui/src/views/OfficeView.tsx` | 修改 — OCR + 转写加 picker 按钮 |
|  | `ui/src/views/OrganizeWizard.tsx` | 修改 — 路径输入加目录选择按钮 |
| **i18n** | `ui/src/i18n/zh.ts`, `ui/src/i18n/en.ts` | 修改 — 新增 3 key |
| **测试** | `ui/src/hooks/useFilePicker.test.ts` | **新增** |

不涉及 Rust 后端、desktop Tauri main.rs、core crate。

## 5. API 契约

### `useFilePicker()` hook

```typescript
export function useFilePicker(): {
  isDesktop: boolean;
  picking: Signal<boolean>;
  error: Signal<string | null>;

  /** 选择目录（desktop=Tauri native，browser=webkitdirectory fallback） */
  pickDirectory(opts?: {
    multiple?: boolean;   // default true
    title?: string;       // 对话框标题
  }): Promise<string[]>;

  /** 选择文件（desktop=Tauri native，browser=hidden <input type="file">） */
  pickFiles(opts?: {
    multiple?: boolean;   // default true
    accept?: string;      // HTML accept 字符串 ".pdf,.md,.txt"
    title?: string;
  }): Promise<{ paths: string[]; files: File[] }>;
};
```

### 文件类型映射（内部）

| accept 字符串 | 消费端 |
|--------------|--------|
| `.json,.vault-profile` | Step5 profile 导入 |
| `.pdf,.md,.txt,.docx,.png,.jpg,.jpeg` | ItemsView 上传 |
| `.pdf,.png,.jpg,.jpeg,.webp,.bmp,.tiff,.tif,.gif` | OfficeView OCR |
| `.bundle,application/octet-stream` | Settings 记忆导入 |
| `audio/*,.wav,.mp3,.m4a,.ogg,.flac` | OfficeView 转写 |

## 6. 扩展点 / 插件接口

- hook 本身是共享基础设施。将来任何新增 UI 页面需要文件/目录选择，直接 import 消费。
- accept→Tauri filter 映射表可扩展（`acceptToTauriFilters()` 函数），新增文件类型只需加一行。
- 如需支持 macOS，`@tauri-apps/plugin-dialog` 的 `open()` API 已跨平台，无需改代码。
- 浏览器 `webkitdirectory` 在 Chromium/Firefox 支持度不同，hook 内部已做能力探测回退。

## 7. 错误 + 边界 case

| 场景 | 行为 |
|------|------|
| Tauri dialog `open()` throw | `error.value = e.message`，返回空数组。消费端 toast 展示错误 |
| 用户取消选择 | 返回 `[]` 空数组，不设 error，不 toast |
| 浏览器不支持 `webkitdirectory` | `error.value = 'not_supported'`，返回 `[]`。消费端 toast 提示"当前浏览器不支持目录选择" |
| `pickFiles` 桌面下返回非空 paths | 消费端用浏览器 `FileReader` / 直接读路径构建 FormData 或传 backend |
| `pickFiles` 浏览器下返回非空 files | 消费端拿 File 对象直接走现有上传流程 |
| 重复点击（picking=true 时再点） | hook 内部不做二次调用，调用者可自行用 `picking` signal 禁用按钮 |
| accept 字符串包含通配 `*` | `acceptToTauriFilters` 正常映射为 `[{name:'All', extensions:['*']}]` |
| `pickFiles` accept 为空 | 不设置文件类型过滤，dialog 显示所有文件 |

错误处理不可吞（silent failure），`error.value` 必须消费端 toast。桌面 picker 失败返回空数组，消费端逻辑不崩溃。

## 8. 成本契约

- **零成本 🆓**：picker hook 全是本地 CPU，毫秒级，无网络请求
- **不触发任何 LLM/API 调用**：仅选择文件/目录，后续上传/绑定等走各自现有成本通道
- **桌面 picker 依赖 @tauri-apps/plugin-dialog**：已在 `apps/attune-desktop/Cargo.toml` 中引入，无新增依赖

## 9. 测试矩阵

| 类型 | 场景 | 方式 |
|------|------|------|
| hook 单元 | `pickDirectory()` Desktop 正常返回 | mock `@tauri-apps/plugin-dialog`，`vitest`+`jsdom` |
| hook 单元 | `pickDirectory()` Desktop 取消 | mock `open()` 返回 null |
| hook 单元 | `pickDirectory()` Desktop 抛异常 | mock `open()` throw Error |
| hook 单元 | `pickFiles()` Desktop 正常返回 | mock `open()` 返回 paths |
| hook 单元 | `pickFiles()` Browser 正常 | jsdom 创建 input, dispatch change |
| hook 单元 | `pickDirectory()` Browser webkitdirectory | jsdom 创建 input, dispatch change |
| hook 单元 | `pickDirectory()` Browser 不支持 | mock webkitdirectory 不可用 |
| hook 单元 | accept→Tauri filter 映射表 | 参数化测试 |
| 组件 | Step5Data 目录 picker 行为不变 | 更新 import，行为无变更 |
| 组件 | OfficeView 转写按钮渲染+点击 | 新增加，mock hook 验证 |
| 手动 | Desktop 真 Native dialog 弹出 | `tests/MANUAL_TEST_CHECKLIST.md` 补充 |
| E2E | Browser 端文件上传流程 | Playwright `fileInput.setInputFiles()` 验证回退 |

## 10. 向后兼容

- **hook 替代内联逻辑**：3 处已有 Tauri dialog 调用的行为完全不变，仅代码位置变化
- **后端 API 不变**：`/upload`、`/index/bind`、`/profile/import`、`/memory/import` 无任何变更
- **i18n key 仅新增**：不修改现有 key，兼容所有 locale
- **浏览器回退路径**：<input type="file"> 回退完全兼容现有浏览器上传行为
- **settings.json** 无变更：不涉及 settings schema

## 11. 风险登记

| 风险 | 缓解 |
|------|------|
| `webkitdirectory` 在 Firefox < 50 不支持 | hook 探测不可用时 `error='not_supported'`，UI toast 提示升级浏览器或用桌面端 |
| Tauri dialog 跨平台行为差异（Windows/Linux 路径分隔符） | `@tauri-apps/plugin-dialog` 保证返回 OS-native path，后端已有 `dunce::canonicalize()` 统一归一化 |
| 多个消费端同时使用 hook | hook 无全局可变状态，每个消费端独立调用，互不影响 |
| 文件选择后桌面 paths 在浏览器 runtime 无法转 File 对象 | hook 只管返回 paths，消费端负责决定是否需读取文件内容。已有 `FileReader` API 可用 |
| Step5Data 既用目录又用文件 picker | hook 提供两种方法互不冲突，Step5Data 内部按 mode 选择调用哪个 |
