import { h, render } from 'preact';

function Popup() {
  return (
    <div style={{ padding: '16px' }}>
      <h2>npu-webhook</h2>
      <p>TODO Phase 2: 连接状态 / NPU状态 / 知识条目数 / 注入开关</p>
    </div>
  );
}

render(<Popup />, document.getElementById('app'));
