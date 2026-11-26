---
name: Sing Shell
description: 紧凑暖色侧栏的 sing-box 桌面客户端
colors:
  background: "#fffdf7"
  ground: "#fbf4e9"
  sidebar: "#f6eddd"
  ink: "#292b26"
  subtle: "#fffefa"
  muted: "#62655a"
  separator: "#d9d9cc"
  accent: "#6752a4"
  primary: "#ffda76"
  primary-hover: "#ffe397"
  navigation: "#f6c4d0"
  hover: "#fff1c6"
  pressed: "#f1dda2"
  success: "#216447"
  success-bg: "#e6f2e8"
  error: "#9a3039"
  error-bg: "#fbe4de"
  disabled-bg: "#eeeade"
  disabled-ink: "#727468"
  primary-border: "#d5b357"
  placeholder: "#6b6d61"
  running-border: "#b2cdb9"
  modal-border: "#aeae9f"
  backdrop: "rgb(31 33 29 / .34)"
typography:
  section:
    fontSize: "16px"
    fontWeight: 650
  brand:
    fontSize: "15px"
    fontWeight: 650
  compact-title:
    fontSize: "22px"
  auxiliary:
    fontSize: "11px"
  title:
    fontFamily: '"Segoe UI Variable", "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif'
    fontSize: "24px"
    fontWeight: 650
    lineHeight: 1.3
    letterSpacing: "-.025em"
  body:
    fontFamily: '"Segoe UI Variable", "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif'
    fontSize: "13px"
    lineHeight: 1.5
  label:
    fontSize: "12px"
    fontWeight: 600
  log:
    fontFamily: '"Cascadia Code", "SFMono-Regular", Consolas, monospace'
    fontSize: "12px"
    lineHeight: 1.7
rounded:
  control: "8px"
  input: "6px"
  modal: "12px"
spacing:
  sm: "8px"
  md: "12px"
  lg: "16px"
  page: "26px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "6px 11px"
    height: "34px"
  input:
    backgroundColor: "{colors.subtle}"
    textColor: "{colors.ink}"
    rounded: "{rounded.input}"
    padding: "6px 9px"
  navigation-current:
    backgroundColor: "{colors.navigation}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
  status-running:
    backgroundColor: "{colors.success-bg}"
    textColor: "{colors.success}"
    rounded: "{rounded.input}"
---

# Design System: Sing Shell

## Overview

**Creative North Star: "多巴胺贝壳"**

保留贝壳标识和暖奶油、淡黄、粉色的品牌语言，采用用户确认的紧凑侧栏方案。日常使用为 Operate 模式，列表、真实运行状态和编辑动作清楚可见。

**Key Characteristics:**
- 暖色侧栏承托粉色当前导航。
- 主区域使用紧凑行、细分隔线和原位详情。
- 黄色主操作、紫色焦点、绿色运行、桃粉错误。

## Colors

纸白主区域与浅米侧栏区分空间；黄色用于明确提交，粉色用于导航选中，浅黄用于筛选。运行与错误同时保留文字。

**The State Color Rule.** 颜色说明状态，文字说明事实；处理中目标不得画成已连接。

## Typography

系统中文无衬线保持桌面阅读习惯。页面标题 24px / 650，区域标题 16px，正文 13px，表单标签 12px，辅助摘要 11px。日志限定使用等宽字体。流量、序号使用表格数字，长名称与路径允许换行。

## Layout

固定侧栏 160px，760px 以下收为 132px；主区域独立滚动，常规留白 20px / 26px，紧凑窗口 16px。列表行最小约 66px，通过内容决定高度。只有设置展开二级，页内筛选和来源详情保持在内容区域。

模态框最大宽度 600px，与窗口四周至少留 16px；头尾固定，内容单独滚动。默认 960×720、最小 620×560 和系统缩放下操作保持可达。

## Elevation & Depth

**The Flat Rule.** 不使用阴影、渐变和装饰动效；用表面颜色、细边框和间距表达层级。原生 dialog 使用半透明墨黑遮罩。

## Shapes

控件使用 6—8px 圆角，模态框 12px；常规边界 1px。播放为圆形按钮，SVG 图标使用一致线宽，不以字符图标替代。仅导航折叠箭头保留短暂旋转，减少动态效果偏好下关闭过渡。

## Components

- 主按钮最小 34px，黄色提交；次按钮纸白，悬停浅黄，禁用灰米。
- 输入使用近白表面，所有键盘焦点采用紫色 3px 外轮廓与 3px 偏移。
- 主导航图标与文字并列，粉色标识当前页；设置二级缩进，选中浅黄。
- 标签为小型浅米摘要，默认标签可见；筛选使用 aria-pressed。
- 模态框使用原生顶层和背景 inert；取消、Esc、关闭保护修改，返回编辑与关闭恢复焦点。
- 活动和来源详情原位展开，不为查看信息弹出额外模态框。

## Do's and Don'ts

- **Do** 复用语义色、细分隔、紧凑行与清晰焦点。
- **Do** 以真实内容和最小窗口检查换行、滚动和操作区。
- **Don't** 新增深色主题、阴影、渐变或装饰动效。
- **Don't** 把原始日志等宽样式扩展为普通界面字体。

依据：src/style.css、src/index.html、modal.ts、nodes.ts、routing.ts 和 assets/icon.svg。
