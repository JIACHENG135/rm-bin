# RM Bin

一个极简的悬浮小图标：拖一张图片上去，就画到你的 reMarkable 上（设备集成开发中，当前为模拟发送）。

## 运行

需要 Rust（`rustup`）和 Node，不要用 sudo。

```sh
npm install
npm run tauri dev     # 开发模式
npm run tauri build   # 打包 .app
```

## 形态与交互

- 116×116 全透明置顶小窗，只有一枚毛玻璃圆角图标，无窗口背景
- 按住图标可拖动位置；悬停右上角出现 ✕ 退出
- 拖入图片：图标放大、蓝色光环、箭头下落动画；非图片红环抖动
- 松手：图标变成图片缩略图 + 进度环 → 白色对勾 → 恢复
- 一次一张图

## 下一步（TODO）

- reMarkable 设备发现与连接
- 真实的图片 → 设备绘制管线（替换 `src-tauri/src/lib.rs` 的 `send_to_remarkable` stub）
