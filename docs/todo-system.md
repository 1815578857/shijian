# 待办系统工作流

## 环境依赖

- **操作系统**：Windows 10/11
- **Node.js**：v18+（用于 todo-server）
- **OpenClaw**：已安装并配置微信插件 `@tencent-weixin/openclaw-weixin`
- **端口**：18800（todo-server）、18789（OpenClaw gateway）

## 架构

```
微信消息 → OpenClaw (qwen3.6-plus) → read/write 工具 → todos.md
                                                          ↑
浏览器 UI (todo-server:18800) ─── API ──────────────────────┘
                                                          ↑
show-briefing.vbs → show-briefing.ps1 (右下角通知) ─── 读取待办数量 ─┘
```

## 关键文件

### 数据与配置

| 文件 | 作用 |
|------|------|
| `E:\AI-notes\memory\todos.md` | 唯一待办数据文件 |
| `E:\AI-notes\skills\todos\SKILL.md` | AI 技能文件（教 AI 如何写待办） |
| `C:\Users\LT\.openclaw\openclaw.json` | OpenClaw 配置（systemPrompt、deny 列表、模型） |

### 服务端

| 文件 | 作用 |
|------|------|
| `C:\Users\LT\.openclaw\todo-server.js` | Node.js 服务端（端口 18800），提供 API + 静态 HTML |
| `C:\Users\LT\.openclaw\todo-ui.html` | Web UI 单页应用（由 todo-server 在 `/` 路由返回） |

### 启动与通知

| 文件 | 作用 |
|------|------|
| `C:\Users\LT\.openclaw\start-todo.vbs` | 静默启动 todo-server（检测端口存活则跳过），然后打开浏览器 |
| `C:\Users\LT\.openclaw\start-gateway.vbs` | 静默启动 OpenClaw gateway（注入 API Key 环境变量） |
| `C:\Users\LT\.openclaw\show-briefing.vbs` | VBS 包装器，静默启动 show-briefing.ps1（避免黑色终端闪烁） |
| `C:\Users\LT\.openclaw\show-briefing.ps1` | 读取待办数量，右下角弹出 WinForms 通知窗口 |
| `C:\Users\LT\.openclaw\install-briefing-task.ps1` | 注册 Windows 计划任务（需管理员权限） |

## 启动/部署流程

### 1. 启动 todo-server

双击 `start-todo.vbs`，它会：
1. 检测 `http://127.0.0.1:18800` 是否已在运行
2. 未运行则 `WshShell.Run "node todo-server.js", 0, False`（静默，无终端窗口）
3. 等待 1.5 秒后自动打开浏览器访问 UI

### 2. 启动 OpenClaw gateway

双击 `start-gateway.vbs`，它会从用户环境变量读取 `KIMI_API_KEY` / `MOONSHOT_API_KEY` 注入到进程，然后静默启动 `gateway.cmd`。

### 3. 注册通知计划任务（一次性）

以管理员权限运行：
```powershell
powershell -ExecutionPolicy Bypass -File "C:\Users\LT\.openclaw\install-briefing-task.ps1"
```

注册后，Windows 会在**用户登录**和**解锁屏幕**时自动触发通知。

### 静默启动原理

所有 VBS 启动脚本使用 `WshShell.Run ..., 0, False`，其中 `0` 表示隐藏窗口。`wscript.exe` 是 GUI 进程，不会创建 console 窗口，因此子进程（node / powershell）也不会出现黑色终端。

## 数据格式

```markdown
- [ ] 待办内容 @c(MM-DD) @d(MM-DD)
- [x] 已完成内容 @c(MM-DD) @d(MM-DD)
```

- `@c(MM-DD)` — 创建日期，必须有
- `@d(MM-DD)` — 截止日期，可选
- `!`/`!!`/`!!!` — 优先级前缀（低/中/高）

## 输入方式

### 微信
发消息给 OpenClaw → AI 自动调用 read/write 写入 todos.md，自动加 `@c`，提到截止时间加 `@d`。

AI 的行为由两处配置控制：
- **SKILL.md**：定义记录流程（read → 追加 → write → 回复）、格式规范、禁止规则
- **openclaw.json 的 systemPrompt**：`"你是笔记机器人。收到非提问消息时，必须调用 read 和 write 工具..."`

### Web UI
访问 `http://127.0.0.1:18800/` → 输入框添加待办（自动加 `@c`），点击截止标签或 📅 图标设置截止时间。

## 截止时间颜色

| 档位 | 条件 | 样式 |
|------|------|------|
| 充裕 | > 7 天 | 绿色 |
| 临近 | 3-7 天 | 橙色 |
| 紧急 | ≤ 3 天 | 红色加粗 |
| 过期 | < 0 天 | 红底白字 |

## API 端点

所有 API 均监听 `127.0.0.1:18800`，请求体为 JSON。

| 方法 | 路径 | 请求体字段 | 说明 |
|------|------|-----------|------|
| GET | `/api/todos` | — | 返回 `{ todos: [...], done: [...] }`，每项含 `text, priority, created, due, line` |
| POST | `/api/todos` | `{ text, priority? }` | 新增待办，自动追加 `@c(今天)` |
| PUT | `/api/todos/complete` | `{ line }` | 将指定行 `- [ ]` 改为 `- [x]` |
| PUT | `/api/todos/uncomplete` | `{ line }` | 将指定行 `- [x]` 改为 `- [ ]` |
| PUT | `/api/todos/edit` | `{ line, newText, priority? }` | 编辑文本，保留原有 `@c`/`@d` 标签 |
| PUT | `/api/todos/priority` | `{ line, priority }` | 修改优先级（0=无, 1/2/3 = !/!!/!!!） |
| PUT | `/api/todos/due` | `{ line, due? }` | 设置截止时间（MM-DD），空值则删除 |
| PUT | `/api/todos/reorder` | `{ fromLine, toLine }` | 拖拽排序，移动行位置 |
| DELETE | `/api/todos` | `{ line }` | 删除指定行 |
| DELETE | `/api/todos/done` | `{ items: [{ line }] }` | 批量删除已完成项 |
| GET | `/api/open` | — | 用系统默认编辑器打开 todos.md |

## 通知机制

### 触发方式
通过 Windows 计划任务 `OpenClaw Briefing` 触发，触发器：
- **用户登录**（延迟 30 秒）
- **解锁屏幕**

### 执行链路
```
计划任务 → wscript.exe show-briefing.vbs → powershell show-briefing.ps1
```
使用 VBS 包装器确保 PowerShell 进程完全静默（无黑色终端闪烁）。

### 行为
1. 检查冷却文件，4 小时内已弹过则直接退出
2. 读取 todos.md 统计未完成数量，为 0 则退出
3. 写入冷却时间戳
4. 右下角弹出 WinForms 通知窗口（280×100，白底，半透明），显示待办数量
5. 10 秒后自动关闭，点击则打开浏览器访问 `http://127.0.0.1:18800`
