/** Attune 主应用根组件（Phase 2 · 组件 demo 版）
 *
 * 此版本展示 Phase 2 产出的 primitives。下一 Phase 接入 wizard + layout。
 */

import { useState, useEffect } from 'preact/hooks';
import { Button, Input, Modal, Drawer, ToastContainer, toast, EmptyState } from './components';
import { t, currentLocale, setLocale } from './i18n';
import { theme, connectionState } from './store/signals';
import { startConnectionMonitor } from './store/connection';

export function App() {
  const [pwd, setPwd] = useState('');
  const [modalOpen, setModalOpen] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);

  // 启动连接监控（future: 会自动 ping /health）
  useEffect(() => {
    startConnectionMonitor();
  }, []);

  // 切主题 attribute
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme.value);
  }, []);

  return (
    <main
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        minHeight: '100vh',
        gap: 'var(--space-5)',
        padding: 'var(--space-6)',
      }}
    >
      <header style={{ textAlign: 'center' }}>
        <h1 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600, marginBottom: 'var(--space-2)' }}>
          🌿 {t('app.name')}
        </h1>
        <p style={{ fontSize: 'var(--text-lg)', color: 'var(--color-text-secondary)' }}>
          {t('app.tagline')}
        </p>
        <p
          style={{
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text-secondary)',
            maxWidth: 560,
            marginTop: 'var(--space-3)',
          }}
        >
          {t('app.promise')}
        </p>
      </header>

      <section
        style={{
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-secondary)',
          display: 'flex',
          alignItems: 'center',
          gap: 'var(--space-2)',
        }}
      >
        <span className={`status-dot ${connectionState.value}`} />
        {t(`conn.${connectionState.value}`)}
        <span style={{ margin: '0 var(--space-2)' }}>·</span>
        Phase 2 · 组件库
        <span style={{ margin: '0 var(--space-2)' }}>·</span>
        Locale: {currentLocale.value}
      </section>

      {/* ─── 组件 demo ─── */}
      <section
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-4)',
          padding: 'var(--space-5)',
          background: 'var(--color-surface)',
          borderRadius: 'var(--radius-lg)',
          boxShadow: 'var(--shadow-md)',
          maxWidth: 480,
          width: '100%',
        }}
      >
        <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
          组件 Primitives
        </h3>

        <Input
          label={t('wizard.pwd.field')}
          type="password"
          value={pwd}
          onInput={(e) => setPwd(e.currentTarget.value)}
          hint="示例 · 实际 wizard 在 Phase 3"
          placeholder="••••••••"
        />

        <div style={{ display: 'flex', gap: 'var(--space-2)', flexWrap: 'wrap' }}>
          <Button variant="primary" onClick={() => toast('success', '保存成功')}>
            Primary
          </Button>
          <Button variant="secondary" onClick={() => toast('info', '这是一条 info')}>
            Secondary
          </Button>
          <Button variant="ghost" onClick={() => toast('warning', '请注意')}>
            Ghost
          </Button>
          <Button variant="danger" onClick={() => toast('error', '出错了')}>
            Danger
          </Button>
        </div>

        <div style={{ display: 'flex', gap: 'var(--space-2)', flexWrap: 'wrap' }}>
          <Button size="sm" onClick={() => setModalOpen(true)}>
            打开 Modal
          </Button>
          <Button size="sm" onClick={() => setDrawerOpen(true)}>
            打开 Drawer
          </Button>
          <Button
            size="sm"
            onClick={() => setLocale(currentLocale.value === 'zh' ? 'en' : 'zh')}
          >
            切 locale ({currentLocale.value === 'zh' ? 'en' : 'zh'})
          </Button>
        </div>
      </section>

      {/* 空状态 demo */}
      <section style={{ width: '100%', maxWidth: 640 }}>
        <EmptyState
          icon="💬"
          title={t('empty.chat.title')}
          description={t('empty.chat.desc')}
          examples={['帮我检索最近的合同', '这份文件讲了什么？', '有类似的先行技术吗']}
          onExampleClick={(ex) => toast('info', `点击：${ex}`)}
        />
      </section>

      <Modal open={modalOpen} onClose={() => setModalOpen(false)} title="示例 Modal">
        <p style={{ marginBottom: 'var(--space-4)' }}>
          Modal 有 focus trap、ESC 关闭、backdrop click 关闭、滚动锁。
        </p>
        <div style={{ display: 'flex', gap: 'var(--space-2)', justifyContent: 'flex-end' }}>
          <Button variant="ghost" onClick={() => setModalOpen(false)}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" onClick={() => setModalOpen(false)}>
            {t('common.confirm')}
          </Button>
        </div>
      </Modal>

      <Drawer open={drawerOpen} onClose={() => setDrawerOpen(false)} title="示例 Drawer">
        <p>
          Drawer 从右侧 slide in，可以拖拽左边界调节宽度。<br />
          ESC 或点背景关闭。<br />
          单层不叠加（见 spec §4）。
        </p>
      </Drawer>

      <ToastContainer />
    </main>
  );
}
