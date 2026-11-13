# 贡献指南

> 本项目于 2026-11-13 停止维护并转为存档，最后发布版本为 v1.3.5。存档仓库不再接受常规 Issue 或 PR，也不提供维护承诺。可遵循 GPLv3 在自己的 Fork 中继续维护。以下约定仅用于理解历史开发流程或经明确授权的后续整理。

## 分支

- 当前工作区只使用 `main` 作为开发与交付分支。
- 常规文档、功能、修复和 CI 改动直接在 `main` 上进行，不创建额外的本地或远程分支。
- 保持 `main` 跟踪 `origin/main`；提交或推送前先确认工作树和远程状态。

## 提交

采用 Conventional Commits：`type(scope): summary`。常用类型为 `feat`、`fix`、`docs`、`refactor`、`test`、`ci` 与 `chore`。摘要使用祈使语气，长度不超过 72 个字符。

## 验证

修改 Vue/Tauri 前端后运行 `npm run build`。修改文档站后运行 `npm --prefix website run build`。涉及 Rust 时还应运行 `cargo fmt --check` 和 `cargo check`。

## Pull Request

- 一个 PR 只解决一个明确的问题。
- 标题遵循 Conventional Commits，并与最近的 `fix:`、`ci:` PR 形式一致。
- 描述说明动机、变更、验证命令、风险或迁移影响。
- 关联 Issue；用户可见变更应附截图或录屏。
- 合并前，所有必需检查必须通过，且不包含无关格式化或生成文件。
