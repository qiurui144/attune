/** Attune 主应用根组件（Phase 1 脚手架版本）
 *
 * 当前只渲染欢迎页以验证 Preact + Vite + Rust embed 链路。
 * 后续 Phase 依次接入：wizard / layout / views / stability 等。
 */
export function App() {
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
      <h1 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600 }}>
        🌿 Attune
      </h1>
      <p
        style={{
          fontSize: 'var(--text-lg)',
          color: 'var(--color-text-secondary)',
          maxWidth: '640px',
          textAlign: 'center',
        }}
      >
        私有 AI 知识伙伴 —— 本地决定，全网增强，越用越懂你的专业。
      </p>
      <div
        style={{
          display: 'flex',
          gap: 'var(--space-3)',
          fontSize: 'var(--text-sm)',
          color: 'var(--color-text-secondary)',
        }}
      >
        <span>
          Phase 1 · 脚手架 · Preact + Vite + Rust <code>include_str!</code>
        </span>
      </div>
      <button
        type="button"
        style={{
          height: 'var(--btn-h-md)',
          padding: '0 var(--space-4)',
          background: 'var(--color-accent)',
          color: 'white',
          border: 'none',
          borderRadius: 'var(--radius-md)',
          fontWeight: 500,
          transition: 'background var(--duration-fast) var(--ease-out)',
        }}
        onMouseEnter={(e) =>
          (e.currentTarget.style.background = 'var(--color-accent-hover)')
        }
        onMouseLeave={(e) =>
          (e.currentTarget.style.background = 'var(--color-accent)')
        }
        onClick={() => alert('Preact + Signals 就位 · 下一步接入 wizard')}
      >
        测试交互
      </button>
    </main>
  );
}
