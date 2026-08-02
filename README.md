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

- 112×148 全透明置顶小窗，画面里是一台迷你 reMarkable Paper Pro，无窗口背景
- 按住机身可拖动位置；窗口上不放任何按钮，右键弹出系统菜单（设置… / 退出）
- 静止时缓慢呼吸，偶尔在屏角画一笔涂鸦；悬停时 Marker 微微滑出
- 拖入图片：纸面抬起并随光标磁性倾斜；非图片则轻轻摇头
- 松手：图片落到 e-ink 屏上，自上而下扫描成墨色 → 墨迹蒸发 → 手写对勾
- 一次一张图

## 设置

`⌘,`、右键菜单，或菜单栏 `RM Bin › 设置…` 打开。

- 独立的原生窗口，背景是 macOS 26 的 Liquid Glass（`NSGlassEffectView`，旧系统自动退回 `NSVisualEffectView`）
- 设置设备地址与端口，失焦或回车即时生效，无需「保存」
- 「检查连接」对设备 SSH 端口做一次 TCP 握手，显示往返延迟或具体失败原因
- 配置存在 `~/Library/Application Support/com.zoe.rmbin/settings.json`，先写临时文件再 rename
- USB 直连时地址固定 10.11.99.1；Wi-Fi 下在设备「设置 › 通用 › 关于本机 › 版权与许可」查看

## 下一步（TODO）

- reMarkable 设备自动发现（mDNS）
- 真实的图片 → 设备绘制管线（替换 `src-tauri/src/lib.rs` 的 `send_to_remarkable` stub）
