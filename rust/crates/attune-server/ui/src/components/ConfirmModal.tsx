/** Attune ConfirmModal · 统一确认对话框 (取代原生 window.confirm)
 *
 * 原生 confirm() 是 OS 级弹窗，与设计系统割裂、按钮文案锁死 OK/Cancel、
 * 焦点与键盘不可控 (a11y 差)。本组件包 Modal + Cancel/Confirm 双按钮，
 * danger 变体用于危险操作 (删除 / 锁库 / 解绑 / wipe)。
 *
 * 提供两种用法：
 *   1. 受控 <ConfirmModal open .../>
 *   2. useConfirm() hook — 命令式 await confirm({...})，最小化迁移 boilerplate
 */

import type { JSX } from 'preact';
import { useCallback, useRef } from 'preact/hooks';
import { signal, useSignal } from '@preact/signals';
import { Modal } from './Modal';
import { Button } from './Button';
import { t } from '../i18n';

export type ConfirmModalProps = {
  open: boolean;
  title: string;
  message: string;
  /** 确认按钮文案；缺省 common.confirm */
  confirmLabel?: string;
  /** 取消按钮文案；缺省 common.cancel */
  cancelLabel?: string;
  /** danger 用红色变体 (删除 / wipe / 解绑等不可逆操作) */
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
};

export function ConfirmModal({
  open,
  title,
  message,
  confirmLabel,
  cancelLabel,
  danger = false,
  onConfirm,
  onCancel,
}: ConfirmModalProps): JSX.Element | null {
  return (
    <Modal open={open} onClose={onCancel} title={title} maxWidth={440} disableBackdropClose>
      <p
        style={{
          fontSize: 'var(--text-sm)',
          color: 'var(--color-text)',
          lineHeight: 1.6,
          margin: 0,
        }}
      >
        {message}
      </p>
      <div
        style={{
          display: 'flex',
          justifyContent: 'flex-end',
          gap: 'var(--space-2)',
          marginTop: 'var(--space-5)',
        }}
      >
        <Button variant="secondary" onClick={onCancel}>
          {cancelLabel ?? t('common.cancel')}
        </Button>
        <Button variant={danger ? 'danger' : 'primary'} onClick={onConfirm}>
          {confirmLabel ?? t('common.confirm')}
        </Button>
      </div>
    </Modal>
  );
}

export type ConfirmOptions = {
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
};

export type UseConfirmResult = {
  /** 命令式调用：返回 Promise；用户点确认 resolve true，取消 / Esc resolve false */
  confirm: (opts: ConfirmOptions) => Promise<boolean>;
  /** 渲染到组件树任意处的 modal 元素 */
  confirmModal: JSX.Element | null;
};

/**
 * 命令式确认 hook。最小化从 `if (!confirm(msg)) return` 的迁移成本：
 *
 * ```tsx
 * const { confirm, confirmModal } = useConfirm();
 * async function onDelete() {
 *   if (!(await confirm({ title, message, danger: true }))) return;
 *   await api.delete(...);
 * }
 * return <>{...} {confirmModal}</>;
 * ```
 */
export function useConfirm(): UseConfirmResult {
  const opts = useSignal<ConfirmOptions | null>(null);
  // resolver 用 ref 而非 signal：纯命令式回调，不参与渲染
  const resolver = useRef<((v: boolean) => void) | null>(null);

  const confirm = useCallback((o: ConfirmOptions): Promise<boolean> => {
    opts.value = o;
    return new Promise<boolean>((resolve) => {
      resolver.current = resolve;
    });
  }, []);

  const settle = useCallback((result: boolean) => {
    opts.value = null;
    resolver.current?.(result);
    resolver.current = null;
  }, []);

  const current = opts.value;
  const confirmModal = current ? (
    <ConfirmModal
      open
      title={current.title}
      message={current.message}
      confirmLabel={current.confirmLabel}
      cancelLabel={current.cancelLabel}
      danger={current.danger}
      onConfirm={() => settle(true)}
      onCancel={() => settle(false)}
    />
  ) : null;

  return { confirm, confirmModal };
}

// ── 全局命令式确认 (Toast 同款单例模式) ──────────────────────────
// 用于 hook 不可达的上下文 (模块级函数 / 多处共用的 lockVault)。
// 单例：同一时刻只渲染一个全局确认框；<ConfirmHost/> 在 App 顶层挂一次。

type GlobalConfirmState = (ConfirmOptions & { resolve: (v: boolean) => void }) | null;
const globalConfirm = signal<GlobalConfirmState>(null);

/** 命令式全局确认。返回 resolve true/false 的 Promise。需 ConfirmHost 已挂载。 */
export function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    // 已有未决确认框时，旧的视为取消，避免悬挂的 Promise
    globalConfirm.value?.resolve(false);
    globalConfirm.value = { ...opts, resolve };
  });
}

/** 全局确认框宿主，App 顶层挂载一次 (与 ToastContainer 并列)。 */
export function ConfirmHost(): JSX.Element | null {
  const cur = globalConfirm.value;
  if (!cur) return null;
  const settle = (result: boolean): void => {
    globalConfirm.value = null;
    cur.resolve(result);
  };
  return (
    <ConfirmModal
      open
      title={cur.title}
      message={cur.message}
      confirmLabel={cur.confirmLabel}
      cancelLabel={cur.cancelLabel}
      danger={cur.danger}
      onConfirm={() => settle(true)}
      onCancel={() => settle(false)}
    />
  );
}
