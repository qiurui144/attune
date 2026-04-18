/** English messages · covers MVP critical path only
 *
 * Non-critical strings fallback to Chinese per i18n/core.ts lookup chain.
 * Per UX Quality spec §2 "English translation strategy".
 */
export const en = {
  // ─── Common ──────────────────────────────────────────
  'common.save': 'Save',
  'common.cancel': 'Cancel',
  'common.delete': 'Delete',
  'common.retry': 'Retry',
  'common.confirm': 'Confirm',
  'common.close': 'Close',
  'common.back': 'Back',
  'common.next': 'Next',
  'common.skip': 'Skip',
  'common.finish': 'Finish',
  'common.loading': 'Loading…',

  // ─── App brand ───────────────────────────────────────
  'app.name': 'Attune',
  'app.tagline': 'Private AI Knowledge Companion',
  'app.promise':
    'Local-first, globally augmented, increasingly attuned to your expertise.',

  // ─── Wizard common ───────────────────────────────────
  'wizard.skip_all': 'Skip all',
  'wizard.step.welcome': 'Welcome',
  'wizard.step.password': 'Password',
  'wizard.step.llm': 'AI',
  'wizard.step.hardware': 'Hardware',
  'wizard.step.data': 'Data',

  // ─── Wizard Step 1 ──────────────────────────────────
  'wizard.welcome.title': 'Attune',
  'wizard.welcome.sub': 'Private AI Knowledge Companion',
  'wizard.welcome.pillar.evolve': 'Active Evolution',
  'wizard.welcome.pillar.evolve_desc': 'Gets smarter the more you use it',
  'wizard.welcome.pillar.companion': 'Conversational',
  'wizard.welcome.pillar.companion_desc': 'Assistant with memory',
  'wizard.welcome.pillar.hybrid': 'Hybrid Intelligence',
  'wizard.welcome.pillar.hybrid_desc': 'Local + Web',
  'wizard.welcome.cta': 'Get started',
  'wizard.welcome.import_existing': 'I have a vault — import backup',

  // ─── Wizard Step 2 · Master Password ────────────────
  'wizard.pwd.heading': 'Set Master Password',
  'wizard.pwd.warning':
    '⚠ Cannot be recovered if forgotten. All data locally encrypted with Argon2id + AES-256-GCM.',
  'wizard.pwd.field': 'Password',
  'wizard.pwd.confirm': 'Confirm',
  'wizard.pwd.show': 'Show',
  'wizard.pwd.hide': 'Hide',
  'wizard.pwd.strength.weak': 'Weak',
  'wizard.pwd.strength.medium': 'Medium',
  'wizard.pwd.strength.strong': 'Strong',
  'wizard.pwd.export_secret':
    'Also generate Device Secret file (for multi-device sync)',
  'wizard.pwd.err.too_short': 'Password must be at least 12 characters',
  'wizard.pwd.err.too_weak': 'Password must contain letters and numbers',
  'wizard.pwd.err.mismatch': 'Passwords do not match',

  // ─── Wizard Step 3 · LLM ────────────────────────────
  'wizard.llm.heading': 'Choose your AI brain',
  'wizard.llm.ollama.title': 'Local Ollama',
  'wizard.llm.ollama.tag': 'Recommended · Free · Private',
  'wizard.llm.ollama.detecting': 'Detecting Ollama…',
  'wizard.llm.ollama.found': '✓ Found Ollama · {models} models',
  'wizard.llm.ollama.missing': 'Ollama not detected',
  'wizard.llm.ollama.install_hint': 'One-click install',
  'wizard.llm.ollama.rescan': 'Rescan',
  'wizard.llm.cloud.title': 'Cloud API',
  'wizard.llm.cloud.tag': 'Bring your own token',
  'wizard.llm.cloud.test': 'Test connection',
  'wizard.llm.skip.title': 'Configure later',
  'wizard.llm.skip.tag': 'Demo mode',
  'wizard.llm.skip.desc': 'Chat disabled, UI browseable',

  // ─── Wizard Step 4 · Hardware ───────────────────────
  'wizard.hw.heading': 'Getting to know your device',
  'wizard.hw.scanning': 'Scanning…',
  'wizard.hw.result.cpu': 'CPU: {model}',
  'wizard.hw.result.gpu': 'GPU: {model}',
  'wizard.hw.result.npu': 'NPU: {model}',
  'wizard.hw.result.ram': 'RAM: {gb} GB',
  'wizard.hw.recommend': 'Based on your hardware, recommended setup:',
  'wizard.hw.model.change': 'Change model',
  'wizard.hw.auto_download': 'Auto-download these models via Ollama',
  'wizard.hw.apply': 'Apply recommendation',

  // ─── Wizard Step 5 · Data ───────────────────────────
  'wizard.data.heading': 'Where to start accumulating?',
  'wizard.data.folder.title': 'Bind a folder',
  'wizard.data.folder.desc': 'Attune will monitor and auto-index',
  'wizard.data.folder.preview': 'Detected {count} indexable files',
  'wizard.data.import.title': 'Import .vault-profile',
  'wizard.data.import.desc': 'From an old device or backup',
  'wizard.data.skip.title': 'Skip for now',
  'wizard.data.skip.desc': 'Add data sources later in Settings',
  'wizard.data.finish': 'Finish · Enter Attune',

  // ─── Wizard · Done page ─────────────────────────────
  'wizard.done.title': 'Welcome to Attune',
  'wizard.done.tip.cmdk': 'Cmd+K to search anything',
  'wizard.done.tip.switch': 'Switch views via sidebar bottom',
  'wizard.done.tip.annotate': 'Select text to annotate',

  // ─── Connection ──────────────────────────────────────
  'conn.online': 'Connected',
  'conn.reconnecting': 'Reconnecting…',
  'conn.offline': 'Server unreachable',
  'conn.retry': 'Retry',

  // ─── Errors ──────────────────────────────────────────
  'error.network': 'Network error: {message}',
  'error.generic': 'Error: {message}',
  'error.vault_locked': 'Vault is locked. Unlock first.',

  // ─── Empty states ───────────────────────────────────
  'empty.chat.title': 'Ask something',
  'empty.chat.desc': 'Based on your knowledge base, or search the web',
  'empty.items.title': 'No items yet',
  'empty.items.desc': 'Drag files or bind a folder',
  'empty.sessions.title': 'Start your first conversation',

  // ─── Shortcuts ──────────────────────────────────────
  'shortcut.search': 'Global search',
  'shortcut.new_chat': 'New chat',
  'shortcut.settings': 'Settings',
  'shortcut.help': 'Shortcuts',
  'shortcut.send': 'Send message',
  'shortcut.close': 'Close',

  // ─── Plural ─────────────────────────────────────────
  'items.count': { one: '{count} item', other: '{count} items' },
} as const;
