# File Manager Picker + UI Style Audit — Summary

**Date:** 2026-07-02
**Branch:** develop
**Range:** 6e2d439..7386a40 (14 commits)

## Spec 1: 所有文档路径支持文件管理器弹出选择 ✅

- Added `useFilePicker` shared hook (`hooks/useFilePicker.ts`)
- 14 vitest unit tests (desktop dialog, cancel, error, wildcard accept, browser fallback, cleanup)
- 3 existing inline Tauri dialog calls migrated to hook (Step5Data, SettingsView, RemoteView)
- 6 new picker buttons/entry points (OfficeView OCR+Transcribe, ItemsView upload, OrganizeWizard, Step5Data import)
- 6 new i18n keys (zh+en, key sets identical per grep guard)
- 15 manual test scenarios added to checklist

## Spec 2: DocIntelView Layout ✅

- Full CSS class → inline style rewrite
- Removed all orphan `class="doc-intel-*"` references (no CSS defined)
- Proper max-width container, OfficeView-pattern tab bar, labeled textareas, cost chip, member gate, 4 result modes

## Spec 3: Global UI Style Audit ✅

- **Style normalization** (7 views): KnowledgeView, SkillsView, ItemsView, RemoteView, ProjectsView, MarketplaceView, WorkbenchView
- **Heavy inline rewrites** (4 views): WritingView (33 CSS classes→inline), MonitoringView (~100 hardcoded px→tokens), SkillRunnerView (~40 px+header added), QuotaView (~40 px→tokens+structure)

## Verification
- `tsc --noEmit`: 0 errors
- `npx vitest run`: 14/14 passed
- i18n zh/en key diff: 0 lines
- No leftover inline `canPickFolder` detection (all from hook)
- No orphan `class=` references (all converted to `style=`)
