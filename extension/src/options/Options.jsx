import { h, render } from 'preact';

function Options() {
  return (
    <div style={{ padding: '24px', maxWidth: '600px', margin: '0 auto' }}>
      <h1>npu-webhook 设置</h1>
      <p>TODO Phase 2: 后端地址 / 注入模式 / 排除域名 / 模型选择 / 数据管理</p>
    </div>
  );
}

render(<Options />, document.getElementById('app'));
