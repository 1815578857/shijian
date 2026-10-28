import { chromium } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';
import http from 'node:http';

const projectDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const executable = process.argv[2] || path.join(projectDir, 'src-tauri', 'target', 'debug', 'shijian-inbox.exe');
if (path.basename(path.dirname(executable)) !== 'debug') throw new Error('原生隔离测试仅运行 debug 程序，请先运行 cargo build；不会启动可能读取真实账号的 release 程序');
const port = 9223;
const endpoint = `http://127.0.0.1:${port}`;
await new Promise((resolve, reject) => {
  const probe = net.createServer();
  probe.once('error', () => reject(new Error(`调试端口 ${port} 已被占用；不会连接其他程序`)));
  probe.listen(port, '127.0.0.1', () => probe.close(resolve));
});
await mkdir(path.join(projectDir, '.artifacts'), { recursive: true });
const testDir = await mkdtemp(path.join(tmpdir(), 'shijian-native-'));
const child = spawn(executable, [], {
  cwd: projectDir,
  env: { ...process.env, SHIJIAN_NATIVE_TEST_DIR: testDir, WEBVIEW2_USER_DATA_FOLDER: path.join(testDir, 'webview'), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1` },
  stdio: ['ignore', 'pipe', 'pipe'],
});
let diagnostics = '';
for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { diagnostics = (diagnostics + chunk.toString()).slice(-4000); });
let spawnError;
child.on('error', error => { spawnError = error; });
let browser;
let modelRequests = 0;
const mockModel = http.createServer((request, response) => {
  if (request.method !== 'GET' || request.url !== '/v1/models' || request.headers.authorization) {
    response.writeHead(400); response.end(); return;
  }
  modelRequests++;
  response.writeHead(200, { 'Content-Type': 'application/json' });
  response.end(JSON.stringify({ data: [{ id: 'local-smoke-model' }] }));
});
await new Promise(resolve => mockModel.listen(0, '127.0.0.1', resolve));
try {
  let ready = false;
  for (let attempt = 0; attempt < 120; attempt++) {
    if (spawnError) throw spawnError;
    if (child.exitCode !== null) throw new Error(`桌面程序提前退出：${child.exitCode}\n${diagnostics}`);
    try { const response = await fetch(`${endpoint}/json/version`); if (response.ok) { ready = true; break; } } catch {}
    await new Promise(resolve => setTimeout(resolve, 500));
  }
  if (!ready) throw new Error('未能连接本次启动的 WebView2 调试端口');
  browser = await chromium.connectOverCDP(endpoint);
  let page;
  for (let attempt = 0; attempt < 30; attempt++) {
    page = browser.contexts().flatMap(context => context.pages()).find(candidate => !candidate.url().startsWith('devtools:'));
    if (page) break;
    await new Promise(resolve => setTimeout(resolve, 300));
  }
  if (!page) throw new Error('没有找到桌面窗口');
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.getByRole('heading', { level: 1, name: '待办', exact: true }).waitFor();
  const snapshot = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('get_snapshot'));
  if (snapshot.runtime !== 'desktop') throw new Error('当前不是原生桌面运行时');
  if (path.resolve(snapshot.dataDir).toLowerCase() !== path.resolve(testDir).toLowerCase()) throw new Error('未启用独立测试数据库，停止测试以免修改真实数据');
  if (snapshot.records.length || snapshot.wechat.accountId) throw new Error('测试环境不应加载真实记录或微信账号');
  await page.screenshot({ path: path.join(projectDir, '.artifacts', 'native-home.png') });
  const title = `桌面验证 ${Date.now()}`;
  await page.getByRole('button', { name: /新建待办/ }).click();
  await page.getByRole('textbox', { name: '待办内容' }).fill(title);
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await page.getByRole('button', { name: new RegExp(`^${title}`) }).click();
  await page.getByRole('textbox', { name: '待办标题' }).fill(`${title} 已编辑`);
  await page.getByRole('textbox', { name: '待办标题' }).press('Enter');
  await page.getByRole('button', { name: new RegExp(`^${title} 已编辑`) }).waitFor();
  await page.screenshot({ path: path.join(projectDir, '.artifacts', 'native-detail.png') });
  await page.getByRole('button', { name: '归档', exact: true }).click();
  await page.getByRole('button', { name: '撤销', exact: true }).click();
  await page.getByRole('button', { name: new RegExp(`^${title} 已编辑`) }).click();
  await page.getByRole('button', { name: '归档', exact: true }).click();
  const removable = '完成删除验证';
  await page.getByRole('button', { name: /新建待办/ }).click();
  await page.getByRole('textbox', { name: '待办内容' }).fill(removable);
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await page.getByRole('button', { name: new RegExp(`^${removable}`) }).click();
  await page.getByRole('button', { name: '标记完成', exact: true }).click();
  const tabs = page.getByRole('group', { name: '记录筛选' });
  await tabs.getByRole('button', { name: /已完成/ }).click();
  await page.getByRole('button', { name: new RegExp(`^${removable}`) }).click();
  await page.getByRole('button', { name: '归档', exact: true }).click();
  await tabs.getByRole('button', { name: /已归档/ }).click();
  await page.getByRole('button', { name: new RegExp(`^${removable}`) }).click();
  await page.getByRole('button', { name: '恢复', exact: true }).click();
  await tabs.getByRole('button', { name: /已完成/ }).click();
  await page.getByRole('button', { name: new RegExp(`^${removable}`) }).click();
  await page.getByRole('button', { name: '重新打开', exact: true }).click();
  await tabs.getByRole('button', { name: /进行中/ }).click();
  await page.getByRole('group', { name: '记录筛选' }).getByRole('button', { name: /进行中/ }).click();
  await page.getByRole('button', { name: new RegExp(`^${removable}`) }).click();
  await page.getByRole('button', { name: '永久删除', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: '永久删除', exact: true }).click();
  await page.getByRole('button', { name: new RegExp(`^${removable}`) }).waitFor({ state: 'detached' });
  await page.getByRole('navigation').getByRole('button', { name: '连接', exact: true }).click();
  await page.getByRole('heading', { name: '微信', exact: true }).waitFor();
  await page.getByRole('button', { name: '连接微信', exact: true }).click();
  await page.getByRole('alert').filter({ hasText: /测试/ }).waitFor();
  if (await page.getByRole('img', { name: /微信连接二维码/ }).count()) throw new Error('隔离测试不应生成真实微信二维码');
  await page.screenshot({ path: path.join(projectDir, '.artifacts', 'native-connections.png') });
  await page.getByRole('textbox', { name: /API 地址/ }).fill(`http://127.0.0.1:${mockModel.address().port}/v1`);
  await page.getByRole('button', { name: '获取模型', exact: true }).click();
  await page.getByRole('dialog').getByRole('option', { name: 'local-smoke-model', exact: true }).click();
  await page.getByRole('button', { name: '保存配置', exact: true }).click();
  await page.getByText('已获取 1 个模型', { exact: true }).waitFor();
  if (modelRequests !== 1) throw new Error('模型测试没有严格限制为一次本机 models 请求');
  await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('save_settings', { patch: { modelBaseUrl: '', modelName: '' } }));
  await page.getByRole('navigation').getByRole('button', { name: '设置', exact: true }).click();
  const panelToggle = page.getByRole('switch', { name: '桌面待办面板' });
  await panelToggle.click();
  let panelPage;
  for (let attempt = 0; attempt < 30; attempt++) {
    panelPage = browser.contexts().flatMap(context => context.pages()).find(candidate => candidate.url().includes('view=panel'));
    if (panelPage) break;
    await new Promise(resolve => setTimeout(resolve, 300));
  }
  if (!panelPage) throw new Error('开启桌面待办面板后没有找到面板窗口');
  panelPage.on('pageerror', error => errors.push(`panel: ${error.message}`));
  for (const windowPage of [page, panelPage]) {
    windowPage.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  }
  await panelPage.getByText('拾笺', { exact: true }).waitFor();
  if (await panelPage.getByText(/实时同步不可用/).count()) throw new Error('面板事件订阅失败');
  await page.getByRole('navigation').getByRole('button', { name: '待办', exact: true }).click();
  await panelPage.getByRole('textbox', { name: '快速添加待办' }).fill('面板冒烟记录');
  await panelPage.getByRole('button', { name: '添加待办' }).click();
  await panelPage.getByText('面板冒烟记录').waitFor();
  // DOM assertion without focusing the receiving window: tests event-driven sync.
  await page.getByRole('button', { name: /^面板冒烟记录/ }).waitFor();
  await page.getByRole('textbox', { name: '添加待办' }).fill('主窗口同步记录');
  await page.getByRole('textbox', { name: '添加待办' }).press('Enter');
  await panelPage.getByText('主窗口同步记录', { exact: true }).waitFor();
  await panelPage.getByRole('button', { name: '完成：主窗口同步记录' }).click();
  await page.getByRole('button', { name: /^主窗口同步记录/ }).waitFor({ state: 'detached' });
  const dragCapability = await panelPage.evaluate(async () => {
    const permissions = await window.__TAURI_INTERNALS__.invoke('plugin:window|start_dragging', { label: 'panel' }).then(() => 'allowed', reason => String(reason));
    return permissions;
  });
  if (dragCapability !== 'allowed') throw new Error(`面板拖动调用失败：${dragCapability}`);
  await panelPage.screenshot({ path: path.join(projectDir, '.artifacts', 'native-panel.png') });
  await page.getByRole('navigation').getByRole('button', { name: '设置', exact: true }).click();
  await panelToggle.click();
  await page.waitForTimeout(600);
  const serialized = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('export_records'));
  const exported = JSON.parse(serialized);
  if (!Array.isArray(exported) || !exported.some(record => record.title === `${title} 已编辑` && record.status === 'archived')) throw new Error('原生数据持久化或导出不正确');
  if (exported.some(record => record.title === removable)) throw new Error('永久删除的记录不应再出现在导出中');
  if (!exported.some(record => record.title === '面板冒烟记录')) throw new Error('面板新增的记录应已写入本地数据库');
  if (errors.length) throw new Error(`窗口运行错误：${errors.join('; ')}`);
  await page.screenshot({ path: path.join(projectDir, '.artifacts', 'native-settings.png') });
  await page.getByRole('button', { name: '退出拾笺', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: '退出拾笺', exact: true }).click();
  await new Promise((resolve, reject) => {
    if (child.exitCode !== null) return resolve();
    const timer = setTimeout(() => reject(new Error('退出后进程未终止')), 10000);
    child.once('exit', () => { clearTimeout(timer); resolve(); });
  });
  console.log(JSON.stringify({ runtime: 'desktop', result: 'passed', checks: ['独立测试数据库与WebView缓存', '真实窗口渲染', 'SQLite新增编辑', '归档撤销', '完成归档恢复与永久删除', '桌面待办面板新增', '主窗口与面板双向事件同步', '面板拖动权限', '真实记录导出', '本机模拟模型列表请求', '测试模式拒绝真实微信连接', '设置界面', '退出仅终止测试子进程'], note: '未读取或修改真实数据库、微信会话和模型密钥；仅调用本机模拟模型服务。' }, null, 2));
} finally {
  await browser?.close().catch(() => {});
  await new Promise(resolve => mockModel.close(resolve));
  if (child.pid && child.exitCode === null && child.signalCode === null) {
    const stopped = new Promise(resolve => child.once('exit', resolve));
    child.kill();
    await stopped;
  }
  await rm(testDir, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 }).catch(() => console.warn('测试临时目录仍被 WebView 占用，可稍后清理：', testDir));
}
