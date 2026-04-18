import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

// 产物：单文件 dist/index.html（JS + CSS + assets 全部 inline）
// 通过 Rust include_str!() 嵌入 attune-server 二进制
export default defineConfig({
  plugins: [preact(), viteSingleFile()],
  build: {
    target: 'es2020',
    cssCodeSplit: false,
    assetsInlineLimit: 100_000_000, // 强制内联全部资产
    chunkSizeWarningLimit: 2048,
  },
  server: {
    port: 5173,
    proxy: {
      // 前端开发时代理 API 到 Rust server
      '/api': 'http://localhost:18900',
      '/ws': { target: 'ws://localhost:18900', ws: true },
    },
  },
});
