# herdr


<p align="center">
  <img src="assets/logo.png" alt="herdr" width="100" />
</p>

<p align="center">
  <a href="https://herdr.dev">herdr.dev</a> · <a href="#安装">安装</a> · <a href="https://herdr.dev/zh-cn/docs/quick-start/">快速开始</a> · <a href="https://herdr.dev/zh-cn/docs/">文档</a></p>

<p align="center">
  <a href="README.md">English</a> · 简体中文
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-666666?labelColor=333333" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/herdrdev/herdr/releases"><img src="https://img.shields.io/github/downloads/herdrdev/herdr/total?labelColor=333333&color=666666" alt="total GitHub release downloads" /></a>
  <a href="https://github.com/herdrdev/herdr/stargazers"><img src="https://img.shields.io/github/stars/herdrdev/herdr?labelColor=333333&color=666666&logo=github" alt="GitHub stars" /></a>
  <a href="https://github.com/herdrdev/herdr/releases/latest"><img src="https://img.shields.io/github/v/release/herdrdev/herdr?label=release&labelColor=333333&color=666666" alt="latest stable release" /></a>
  <a href="https://formulae.brew.sh/formula/herdr"><img src="https://img.shields.io/homebrew/v/herdr?label=homebrew&labelColor=333333&color=666666" alt="Homebrew version" /></a>
  <a href="https://x.com/herdrdev"><img src="https://img.shields.io/badge/follow-%40herdrdev-000000?logo=x&logoColor=white" alt="follow @herdrdev on X" /></a>
</p>

---

https://github.com/user-attachments/assets/043ec09f-4bdd-41d5-aee0-8fda6b83e267

**智能体复用器，住在你的终端里。**

- **每个智能体一目了然**——`blocked`、`working`、`done`。真实的终端视图，而不是包装过的转述。
- **分离后工作继续运行**——关闭客户端或 SSH 断线后，后台服务器仍会保持终端运行。服务器或机器重启后，Herdr 会恢复已保存的布局，并可恢复受支持的智能体会话；原有进程不会保留。[会话状态 →](https://herdr.dev/zh-cn/docs/session-state/)
- **多台机器，一个窗口**——将本地工作和已保存的 SSH 机器放在一起，使用汇总的智能体列表，各连接独立重连。[远程机器 →](https://herdr.dev/zh-cn/docs/connecting-machines/)
- **智能体也能使用 herdr**——纯 socket api：智能体可以创建窗格、读取输出、互相等待。[智能体技能 →](https://herdr.dev/zh-cn/docs/agent-skill/) 在开发智能体？[为你的智能体添加 herdr 支持 →](https://herdr.dev/zh-cn/docs/add-herdr-support/)
- **键盘和鼠标都是一等公民**——tmux 风格的前缀键，*以及*点击、拖动、分割。按当下的场景选择，而不是被工具锁死。
- **插件**——扩展窗格和工作流。[浏览插件市场 →](https://herdr.dev/plugins/)
- **单个 rust 二进制，没有 electron**——运行在你已经在用的任何终端里。

---

## 安装

```bash
curl -fsSL https://herdr.dev/install.sh | sh
```

或者 `brew install herdr` · `mise use -g herdr` · Windows：`powershell -ExecutionPolicy Bypass -c "irm https://herdr.dev/install.ps1 | iex"` · [受端点保护的 Windows](https://herdr.dev/zh-cn/docs/windows-beta/) · [二进制文件](https://github.com/herdrdev/herdr/releases)

然后在工作所在的目录启动它：

```bash
herdr
```

运行你的智能体、分割窗格，然后安心离开。`ctrl+b q` 分离，`herdr` 重新连接。[快速开始 →](https://herdr.dev/zh-cn/docs/quick-start/)

在 `[ui]` 下设置 `rounded_borders = true`，即可为现有界面边框启用圆角。默认保持不变；终端内容、尺寸和共享连接处均保持原样。参见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

在 `[ui]` 下设置 `pane_focus_weight = true`，即可启用不依赖颜色的焦点提示：获得焦点的窗格边框在强调色之外改用粗线字符。默认保持不变，窗格尺寸和分隔线均保持原样。参见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

在 `[ui]` 下用 `pane_gap_cells = N` 设置分割窗格之间的空白：`0` 共用分隔线，`N` 在窗格边框之间留出 N 个空白单元格。未设置时保持现有的 `pane_gaps` 行为。参见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

在 `[ui]` 下设置 `pane_padding_cells = 1`，可在每个窗格的边框与内容之间留出空白单元格。默认值 `0` 保持原有布局。参见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

在 `[ui]` 下用 `sidebar_padding_cells = 1` 为侧边栏留出空间：每个展开的分区及其点击区域向内收缩，侧边栏宽度不变。默认值 `0` 保持现有布局。参见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

在 `[ui]` 下用 `pane_title_tokens` 以与智能体侧边栏相同的样式化标记组成窗格边框标题，包括实时的 `$name` 窗格元数据。未设置时保持现有标题。参见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

在 `[ui]` 下用 `pane_border_identity_token` 根据一个样式化标记为未获得焦点的窗格边框线着色，例如通过窗格元数据报告的 `$role`。获得焦点的窗格保持强调色，未设置时保持现有边框颜色。

在 `[ui]` 下用 `inactive_pane_dim_percent = N`（0-100）淡化未聚焦窗格的文字。它只在普通终端模式下、仅在屏幕上把文字颜色向终端背景色混合，背景色、边框和窗格内容保持不变。参见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

## 文档

所有文档都在 [herdr.dev/docs](https://herdr.dev/zh-cn/docs/)：[快速开始](https://herdr.dev/zh-cn/docs/quick-start/) · [核心概念](https://herdr.dev/zh-cn/docs/concepts/) · [受支持的智能体](https://herdr.dev/zh-cn/docs/agents/) · [键盘](https://herdr.dev/zh-cn/docs/keyboard/) · [配置](https://herdr.dev/zh-cn/docs/configuration/) · [会话状态](https://herdr.dev/zh-cn/docs/session-state/) · [连接机器](https://herdr.dev/zh-cn/docs/connecting-machines/) · [远程访问](https://herdr.dev/zh-cn/docs/persistence-remote/) · [集成](https://herdr.dev/zh-cn/docs/integrations/) · [为智能体添加 herdr 支持](https://herdr.dev/zh-cn/docs/add-herdr-support/) · [插件](https://herdr.dev/zh-cn/docs/plugins/) · [socket api](https://herdr.dev/zh-cn/docs/socket-api/)

可在 `[theme.custom]` 中使用可选的 `active_tab_fg` 和 `active_tab_bg` 自定义已聚焦标签页标签的颜色（启用 `auto_switch` 时也可在 `light`/`dark` 子表中设置）。省略颜色会保留现有的动态默认值；重置别名明确使用终端默认色。详情见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

可在 `[theme.custom]` 中使用可选的 `pane_border_active` 和 `pane_border_inactive` 自定义窗格边框与标题的聚焦颜色（启用 `auto_switch` 时也可在 `light`/`dark` 子表中设置）。省略时继续跟随 `accent` 和 `overlay0`；重置别名明确使用终端默认色。详情见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

可在 `[theme.custom]` 中使用可选的 `popup_bg` 和 `popup_border` 自定义终端弹窗（启用 `auto_switch` 时也可在 `light`/`dark` 子表中设置）。省略颜色会保留现有的弹窗外观；弹窗内程序显式设置的背景色保持不变。详情见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

可在 `[theme.custom]` 中使用可选的 `pane_inactive_bg` 为未聚焦窗格着色（启用 `auto_switch` 时也可在 `light`/`dark` 子表中设置）。只会改变终端默认背景；程序颜色、反色显示、选区和已聚焦窗格保持不变。省略时保持现有外观。详情见[配置](https://herdr.dev/zh-cn/docs/configuration/)。

## 致谢

<a href="https://terminaltrove.com/"><img src="assets/sponsors/terminal-trove.png" alt="Terminal Trove" width="200" /></a>

[Terminal Trove](https://terminaltrove.com/) 以及 [SPONSORS.md](./SPONSORS.md) 中列出的每一位支持者——谢谢 🐑

企业/合作：hey@herdr.dev

## 智能体须知

如果你是协助本仓库的 AI 智能体：在改动代码前阅读 [`AGENTS.md`](./AGENTS.md)，在创建 issue 或 PR 前阅读 [`CONTRIBUTING.md`](./CONTRIBUTING.md)。

## 开发

```bash
git clone https://github.com/herdrdev/herdr
cd herdr
cargo build --release

just test        # 单元测试
just check       # 格式检查、测试和维护性检查
```

## 许可证

herdr 基于 [Apache License 2.0](LICENSE) 许可证发布。
