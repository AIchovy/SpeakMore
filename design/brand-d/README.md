# SpeakMore · D「双声」

用户选定 D 后整理的矢量定稿。保留两枚相向的声部与中间流动留白；使用同一个贝塞尔路径旋转 180° 构造另一半，黑白两版共用完全相同的轮廓。实色填充，无滤镜、阴影、纹理或嵌入位图。

- `mark-light.svg`：透明底黑色标识，用于浅色背景。
- `mark-dark.svg`：透明底白色标识，用于深色背景。
- `app-icon-light.svg`：白底黑标 macOS 图标，外围透明，1024 × 1024。
- `app-icon-dark.svg`：黑底白标 macOS 图标，外围透明，1024 × 1024。
- `light/`、`dark/`：各尺寸 PNG 及 macOS ICNS。
- `preview.svg`、`preview/1024x1024.png`：定稿预览。

主色只使用 #000000 和 #FFFFFF，预览页底色不属于 Logo。矢量稿可无损缩放；两色互换时不改变形状。最小使用建议 16 px，图形外围留白不小于标识宽度的 1/8；macOS 图标的系统留白已包含在画布内。

概念由内置 image_gen 生成，本次定稿依据选中的 D 重新构造为原生 SVG，并通过本地 Tauri 图标工具导出，不是自动描摹位图。原概念与提示词见 ../logo-concepts/README.md。

2026-09-17 已应用于 SpeakMore：默认应用图标使用白底黑标，侧栏与加载页使用跟随主题的黑白标识，菜单栏使用透明模板图标，浏览器预览使用同款 favicon。

应用源图为 `resources/app-icon.svg`，图标导出到 `src-tauri/icons/`；界面组件为 `src/components/brand.tsx`。重新导出命令（在项目根目录执行）：

```sh
npx tauri icon resources/app-icon.svg -o src-tauri/icons
npx tauri icon design/brand-d/mark-light.svg -o /tmp/speakmore-tray-d -p 36
cp /tmp/speakmore-tray-d/36x36.png src-tauri/icons/tray.png
```
