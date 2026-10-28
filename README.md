# 拾笺 · 微信待办助手

基于 **Tauri 2 + React + Rust + SQLite** 的 Windows 本地待办应用，支持手动记录、桌面面板、到期提醒，以及实验性的微信纯文本接收与模型字段解析。

实际应用位于 [`apps/wechat-inbox`](apps/wechat-inbox/README.md)。开发环境、构建、测试、数据目录与凭据管理详见该应用文档。

## 快速开始

需要 Windows 10/11、WebView2、Node.js 22.12+、Rust stable 和 MSVC C++ 构建工具。

```powershell
cd apps/wechat-inbox
npm ci
npm run desktop
```

仅预览界面：`npm run dev`。浏览器预览与桌面版分开，数据仅保存在当前标签页，不连接微信、不保存模型密钥。

## 验证与构建

在应用目录运行：

```powershell
npm test
npm run build
npm run test:e2e
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

桌面安装包：`npm run desktop:build`。原生隔离验证步骤见[应用测试说明](apps/wechat-inbox/README.md#测试)。真实微信接入与账号准入需要独立实测，不能由本机模拟测试证明。

## 文档

- [应用功能、运行与数据管理](apps/wechat-inbox/README.md)
- [旧待办系统方案](docs/todo-system.md)
- [mini-D5 原始方案（历史文档，不代表当前代码）](docs/mini-d5-original-plan.md)

账号资料、数据库、导出和构建产物不应纳入版本管理。忽略规则不等于秘密存储；请勿将实际凭据放入源码或上传。
