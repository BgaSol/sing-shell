---
name: Sing Shell
description: 沿用 SoDeck 多巴胺语言的轻量 sing-box 桌面壳
colors:
  background: "#fbf1e8"
  ink: "#20211f"
  surface: "#fffbe6"
  subtle: "#fffef5"
  muted: "#585c52"
  separator: "#cfccba"
  accent: "#5346a6"
  primary: "#ffde75"
  primary-hover: "#ffe795"
  navigation: "#ffb7cf"
  hover: "#fff0bc"
  pressed: "#f5dea3"
  success: "#176b4f"
  success-bg: "#d5f1e5"
  error: "#a12c38"
  error-bg: "#ffd7cc"
  disabled-bg: "#ece8d9"
typography:
  title:
    fontFamily: '"Segoe UI Variable", "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif'
    fontSize: "16px"
    fontWeight: 700
  body:
    fontFamily: '"Segoe UI Variable", "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif'
    fontSize: "13px"
    lineHeight: 1.6
  label:
    fontSize: "12px"
    fontWeight: 600
  log:
    fontFamily: '"Cascadia Code", "SFMono-Regular", Consolas, monospace'
    fontSize: "12px"
    lineHeight: 1.8
rounded:
  control: "8px"
  panel: "12px"
spacing:
  sm: "8px"
  md: "12px"
  lg: "16px"
  section: "18px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "7px 13px"
  input:
    backgroundColor: "{colors.subtle}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "8px 10px"
  navigation-current:
    backgroundColor: "{colors.navigation}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
  status-running:
    backgroundColor: "{colors.success-bg}"
    textColor: "{colors.success}"
    rounded: "{rounded.control}"
    padding: "7px 11px"
  panel:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.panel}"
    padding: "18px 22px"
---

# Design System: Sing Shell

## Overview

**Creative North Star: "多巴胺贝壳"**

沿用 SoDeck 多巴胺语言的轻量 sing-box 桌面壳。日常应用采用 Operate 模式：状态清楚、操作直接、配置与日志易读。固定浅色，以暖奶油底色承托淡黄面板。

**Key Characteristics:**
- 粉底黄贝壳、绿底座的品牌标识。
- 墨黑结构线、平面色块、紧凑的操作密度。
- 黄色主操作、粉色当前导航、紫色焦点、绿色运行、桃粉错误。

## Colors

主色用于启动、保存等明确操作；粉色标识当前导航。奶油背景、淡黄面板和近白控件区分层级，墨黑承担正文与结构，灰绿承担辅助说明。

**The State Color Rule.** 运行使用绿色，失败使用桃粉底和深红文字；处理中使用黄色底和紫色文字。状态同时保留文字，不只依赖颜色。

## Typography

界面使用系统中文无衬线字体。品牌标题（25px / 700）、区域标题（16px / 700）、字段标题（14px / 650）构成层级；正文与按钮（13px），提示与状态（12px）。日志使用等宽字体及表格数字，长路径和日志允许折行。

## Layout

内容单列，最大宽度（1080px）。首屏依次为品牌与状态、运行／软件更新导航、运行配置、运行日志。主区域间距使用 section；桌面边距（24px 32px 16px）。窄于（680px）时边距收为（18px 16px 16px），区域间距改用 lg，标题和操作可换行。

## Elevation & Depth

**The Flat Rule.** 面板与控件保持平面，不使用阴影、渐变或装饰动效；依靠底色、边框和留白表达层级。对话框使用半透明墨黑遮罩。

## Shapes

区域与对话框使用 panel 圆角，按钮、输入框、状态和日志使用 control 圆角。主要边框（2px），内部细分隔（1px）。图标使用圆端点线性 SVG，品牌保留贝壳轮廓。

## Components

- **按钮：** 黄色主操作、近白次操作；悬停变浅黄，按下变深黄；禁用呈灰绿。最小高度（36px）。
- **输入：** 近白底、墨黑边框；占位提示可读。所有键盘焦点使用紫色外轮廓（3px），向外偏移（3px）。
- **导航：** 图标与文字并列；当前页粉底实线框，其余透明底。
- **状态：** 小圆点与文字并列；状态语义见 Colors。
- **面板与日志：** 淡黄面板，日志内层近白；日志可滚动、可键盘聚焦，区域随可用高度伸展。

## Do's and Don'ts

- **Do** 保留品牌／状态、导航、配置、日志的操作顺序。
- **Do** 使用既有语义色，并保留文字状态与清晰键盘焦点。
- **Don't** 添加深色模式、阴影、渐变或装饰动效。
- **Don't** 把日志等宽字体扩展为普通界面字体。

依据：`src/style.css`、`src/index.html`、`assets/icon.svg`；产品边界以 `AGENTS.md` 与 `README.md` 为准。
