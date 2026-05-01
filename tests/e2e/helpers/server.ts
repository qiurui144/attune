/**
 * attune-server-headless lifecycle helper for E2E.
 *
 * Spawns a fresh server with isolated tempdir vault for each test run, waits
 * until /api/v1/status/health responds 200, then returns base URL + cleanup fn.
 *
 * Per docs/TESTING.md §1.1 E2E layer definition: "Playwright Chrome — 真实浏览器交互".
 * The server is part of test fixtures; no `webServer` in playwright.config.ts so
 * each test can override env vars (e.g., ATTUNE_FORM_FACTOR for F-09 scenarios).
 */
import { spawn, type ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

export interface ServerHandle {
  baseUrl: string;
  proc: ChildProcess;
  tmpDir: string;
  cleanup: () => Promise<void>;
}

export interface SpawnOptions {
  /** Port to bind. If undefined, picks a free ephemeral port. */
  port?: number;
  /** Forwarded as --no-auth flag. Default true for E2E (simpler test setup). */
  noAuth?: boolean;
  /** Set ATTUNE_FORM_FACTOR env var (test F-09 form_factor split). */
  formFactor?: 'laptop' | 'k3' | 'server';
  /** Path to attune-server-headless binary. Default: rust/target/release/attune-server-headless */
  binary?: string;
}

export async function spawnAttuneServer(opts: SpawnOptions = {}): Promise<ServerHandle> {
  const port = opts.port ?? 18901;
  const baseUrl = `http://127.0.0.1:${port}`;
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'attune-e2e-'));

  const binary =
    opts.binary ?? path.resolve(__dirname, '../../../rust/target/release/attune-server-headless');

  if (!fs.existsSync(binary)) {
    throw new Error(
      `attune-server-headless binary not found at ${binary}. ` +
        `Run 'cd rust && cargo build --release' first.`
    );
  }

  const env: NodeJS.ProcessEnv = {
    ...process.env,
    HOME: tmpDir,
    XDG_DATA_HOME: path.join(tmpDir, 'data'),
    XDG_CONFIG_HOME: path.join(tmpDir, 'config'),
  };
  if (opts.formFactor) {
    env.ATTUNE_FORM_FACTOR = opts.formFactor;
  } else {
    delete env.ATTUNE_FORM_FACTOR;
  }

  const args = ['--host', '127.0.0.1', '--port', String(port)];
  if (opts.noAuth !== false) {
    args.push('--no-auth');
  }

  const proc = spawn(binary, args, {
    env,
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  // Capture logs to tmp file for debugging on failure
  const logPath = path.join(tmpDir, 'server.log');
  const logStream = fs.createWriteStream(logPath);
  proc.stdout?.pipe(logStream);
  proc.stderr?.pipe(logStream);

  // Wait for /health to respond (max 30s)
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) {
      const log = fs.readFileSync(logPath, 'utf-8');
      throw new Error(`server exited early (code ${proc.exitCode}). Log:\n${log}`);
    }
    try {
      const res = await fetch(`${baseUrl}/api/v1/status/health`);
      if (res.ok) break;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 200));
  }

  if (Date.now() >= deadline) {
    proc.kill();
    throw new Error(`server at ${baseUrl} did not become ready in 30s`);
  }

  const cleanup = async () => {
    proc.kill('SIGTERM');
    // Give 2s for graceful shutdown
    await new Promise((r) => setTimeout(r, 2_000));
    if (proc.exitCode === null) {
      proc.kill('SIGKILL');
    }
    try {
      fs.rmSync(tmpDir, { recursive: true, force: true });
    } catch {
      // best-effort
    }
  };

  return { baseUrl, proc, tmpDir, cleanup };
}
