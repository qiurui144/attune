/** 简体中文消息 · 主源语言 */
export const zh = {
  // ─── 通用 ──────────────────────────────────────────────
  'common.save': '保存',
  'common.cancel': '取消',
  'common.delete': '删除',
  'common.retry': '重试',
  'common.confirm': '确认',
  'common.close': '关闭',
  'common.back': '返回',
  'common.next': '下一步',
  'common.skip': '跳过',
  'common.finish': '完成',
  'common.loading': '加载中…',

  // ─── App 品牌 ──────────────────────────────────────────
  'app.name': 'Attune',
  'app.tagline': '私有 AI 知识伙伴',
  'app.promise': '本地决定，全网增强，越用越懂你的专业。',

  // ─── Wizard · 通用 ────────────────────────────────────
  'wizard.skip_all': '跳过全部',
  'wizard.step.welcome': '欢迎',
  'wizard.step.password': '密码',
  'wizard.step.llm': 'AI',
  'wizard.step.hardware': '硬件',
  'wizard.step.data': '数据',

  // ─── Wizard Step 1 · 欢迎 ─────────────────────────────
  'wizard.welcome.title': 'Attune',
  'wizard.welcome.sub': '私有 AI 知识伙伴',
  'wizard.welcome.pillar.evolve': '主动进化',
  'wizard.welcome.pillar.evolve_desc': '越用越懂你',
  'wizard.welcome.pillar.companion': '对话伙伴',
  'wizard.welcome.pillar.companion_desc': '有记忆的助手',
  'wizard.welcome.pillar.hybrid': '混合智能',
  'wizard.welcome.pillar.hybrid_desc': '本地 + 联网',
  'wizard.welcome.cta': '开始设置',
  'wizard.welcome.import_existing': '我已有 vault，导入备份',

  // ─── Wizard Step 2 · Master Password ──────────────────
  'wizard.pwd.heading': '设置 Master Password',
  'wizard.pwd.warning': '⚠ 忘记无法找回。所有数据本地 Argon2id + AES-256-GCM 加密。',
  'wizard.pwd.field': '密码',
  'wizard.pwd.confirm': '再次输入',
  'wizard.pwd.show': '显示',
  'wizard.pwd.hide': '隐藏',
  'wizard.pwd.strength.weak': '弱',
  'wizard.pwd.strength.medium': '中',
  'wizard.pwd.strength.strong': '强',
  'wizard.pwd.export_secret': '同时生成 Device Secret 导出文件（多设备同步用）',
  'wizard.pwd.err.too_short': '密码至少需要 12 个字符',
  'wizard.pwd.err.too_weak': '密码需包含字母和数字',
  'wizard.pwd.err.mismatch': '两次输入不一致',

  // ─── Wizard Step 3 · LLM ──────────────────────────────
  'wizard.llm.heading': '选择 AI 大脑',
  'wizard.llm.ollama.title': '本地 Ollama',
  'wizard.llm.ollama.tag': '推荐 · 免费 · 隐私',
  'wizard.llm.ollama.detecting': '检测 Ollama…',
  'wizard.llm.ollama.found': '✓ 发现 Ollama · {models} 个模型',
  'wizard.llm.ollama.missing': '未检测到 Ollama',
  'wizard.llm.ollama.install_hint': '一键安装',
  'wizard.llm.ollama.rescan': '重新扫描',
  'wizard.llm.cloud.title': '云端 API',
  'wizard.llm.cloud.tag': '需自备 token',
  'wizard.llm.cloud.test': '测试连接',
  'wizard.llm.skip.title': '暂不配置',
  'wizard.llm.skip.tag': '演示模式',
  'wizard.llm.skip.desc': 'Chat 禁用，可浏览界面',

  // ─── Wizard Step 4 · 硬件 ─────────────────────────────
  'wizard.hw.heading': '认识你的设备',
  'wizard.hw.scanning': '扫描中…',
  'wizard.hw.result.cpu': 'CPU: {model}',
  'wizard.hw.result.gpu': 'GPU: {model}',
  'wizard.hw.result.npu': 'NPU: {model}',
  'wizard.hw.result.ram': 'RAM: {gb} GB',
  'wizard.hw.recommend': '根据你的硬件，推荐以下搭配',
  'wizard.hw.model.change': '换模型',
  'wizard.hw.auto_download': '让 Ollama 自动下载这些模型',
  'wizard.hw.apply': '应用推荐',

  // ─── Wizard Step 5 · 数据 ─────────────────────────────
  'wizard.data.heading': '从哪里开始积累？',
  'wizard.data.folder.title': '绑定文件夹',
  'wizard.data.folder.desc': 'Attune 会监听此目录变化，自动索引',
  'wizard.data.folder.preview': '检测到 {count} 个可索引文件',
  'wizard.data.import.title': '导入 .vault-profile',
  'wizard.data.import.desc': '来自旧设备或备份',
  'wizard.data.skip.title': '跳过，先看看',
  'wizard.data.skip.desc': '之后随时在 Settings 添加',
  'wizard.data.finish': '完成 · 进入 Attune',

  // ─── Wizard · 完成页 ──────────────────────────────────
  'wizard.done.title': 'Welcome to Attune',
  'wizard.done.tip.cmdk': 'Cmd+K 搜索任何东西',
  'wizard.done.tip.switch': '左栏底部切换视图',
  'wizard.done.tip.annotate': '选中文字自动出现批注按钮',

  // ─── 连接状态 ──────────────────────────────────────────
  'conn.online': '已连接',
  'conn.reconnecting': '重连中…',
  'conn.offline': '服务器未响应',
  'conn.retry': '重试',

  // ─── 错误 ──────────────────────────────────────────────
  'error.network': '网络错误：{message}',
  'error.generic': '出错了：{message}',
  'error.vault_locked': 'Vault 已锁定，请先解锁',

  // ─── 空状态 ───────────────────────────────────────────
  'empty.chat.title': '问点什么吧',
  'empty.chat.desc': '基于你的知识库，或搜索全网',
  'empty.items.title': '还没有录入内容',
  'empty.items.desc': '拖拽文件或绑定文件夹',
  'empty.sessions.title': '开始第一次对话',

  // ─── 快捷键 ───────────────────────────────────────────
  'shortcut.search': '全局搜索',
  'shortcut.new_chat': '新对话',
  'shortcut.settings': '设置',
  'shortcut.help': '快捷键',
  'shortcut.send': '发送消息',
  'shortcut.close': '关闭',

  // ─── Plural 示例 ──────────────────────────────────────
  'items.count': { one: '{count} 条', other: '{count} 条' }, // 中文单复数同形
} as const;

export type MessageKey = keyof typeof zh;
