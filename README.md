# RM Bin

一个极简的悬浮小图标：拖一张图片上去，它就用笔画到你的 reMarkable 上——而窗口里的小屏幕跟着一笔一笔画出同一幅。

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
- 松手：图片落到 e-ink 屏上，然后**设备上每落一笔，屏上就画出同一笔**——照片一边褪成灰、一边被线稿取代 → 墨迹蒸发 → 手写对勾
- 一次一张图
- 画失败（设备连不上、图上描不出线条）时不写对勾，改成摇头

## 绘制管线

设备没有能用的导入接口，但笔的数字化仪 `/dev/input/eventN` 会照单全收任何人写进去的事件——所以就假装成那支笔。这是 [rm-agent](../rm-agent) 的老办法，区别在于 rm-agent 跑在设备上写本地文件，rm-bin 跑在 Mac 上写进一条 ssh 管道：

```
图片 → 二值化 → 骨架化 → 描成折线 → 按横带排序 → input_event 字节 → ssh → dd > /dev/input/eventN
                                    │                                      │
                            draw-plan（同一批折线）              draw-progress（已落笔数）
                                    └──────────────┬───────────────────────┘
                                                   ▼
                                          窗口里逐笔画出同一幅画
```

- **描线**（[`src-tauri/src/rm/imageproc.rs`](src-tauri/src/rm/imageproc.rs)）是 Otsu 阈值 + Zhang-Suen 细化 + 骨架追踪，从 rm-agent 原样搬过来的，改动可以直接互相 diff
- **设备差异**（[`src-tauri/src/rm/device.rs`](src-tauri/src/rm/device.rs)）在 rm-agent 里是 `#[cfg(target_pointer_width)]`，因为它只为一台设备编译；rm-bin 面对的是「插上来的那台」，所以同样的事实得变成运行时的值——`struct input_event` 在 rM2 的 32 位内核上是 16 字节、Paper Pro 上是 24 字节，笔的设备节点和量程也不同。落笔前 `uname -m` 问一句就都定了
- **同步**（[`src-tauri/src/rm/draw.rs`](src-tauri/src/rm/draw.rs)）是这次真正新写的部分。第一版是**让画去迁就窗口**：为了让那道自上而下的扫描线诚实，把折线按横带切开——真机上切口看得见（见下），而且只换来一个近似。现在反过来，**让窗口去迁就画**：折线整条发出，同时把同一批折线（图像坐标、抽稀量化过）通过 `draw-plan` 给前端，`draw-progress` 再报「已落笔数」（带小数，所以笔尖正在走的那一笔是半画出来的）。前端就按同样的顺序、同样的时刻把同样的线画在小屏上。什么都不用切，对应关系也不再是近似
- **横带**只剩排序作用：大致自上而下画比按追踪器发现的顺序画看起来更像有意为之，仅此而已
- **节流即时钟**（`device::push`）：一次性灌进去会让 xochitl 打出 "Dropped pen event!" 并把笔的状态搞乱，所以按 20 个事件 / 4ms 发。副作用是「已写出的字节」很接近「已落在纸上的墨」，进度才有得可报——也意味着一次绘制要几秒到十几秒，窗口里就诚实地画那么久

需要 Mac 能免密 ssh 到 `root@设备地址`（`ssh-copy-id` 一次即可；密码在设备的「设置 › 通用 › 关于本机 › 版权与许可」里）。

### 真机上踩到的三件事

都在 Paper Pro（`reMarkable Ferrari`，固件 3.2x）上验过，也都写进了代码注释：

- **`cat > /dev/input/eventN` 在 Paper Pro 上根本不能用。** 内核只接受整数个 `struct input_event` 的写入，而写多少是由远端那个程序的缓冲区决定的：busybox `cat` 一次写 4096 字节，能被 rM2 的 16 整除，却除不尽 Paper Pro 的 24——于是第一块之后就 EINVAL。Python 老原型只跑 rM2，所以从没撞上。现在远端用 `dd bs=… iflag=fullblock`，`iflag` 是关键的那一半：没有它 dd 会把管道给多少就写多少，照样错位
- **穿过工具栏的笔画不是线，是按钮。** 第一次满版测试图确实画出来了，同时还把页面转成横向、打开了溢出菜单、加了一页。所以留了 10% 的页边距（工具栏停在任意一边都躲得开），并且在落笔前先悬停 400ms 等 xochitl 把工具栏收起来
- **切笔画会看出来。** xochitl 给每笔两端做收尾渐细，一条竖线被切成多段之后边缘是锯齿状的。这条直接导致了上面那次「让窗口迁就画」的重做——现在一笔都不切，锯齿也就不存在了

## 设置

`⌘,`、右键菜单，或菜单栏 `RM Bin › 设置…` 打开。

- 独立的原生窗口，背景是 macOS 26 的 Liquid Glass（`NSGlassEffectView`，旧系统自动退回 `NSVisualEffectView`）
- 设置设备地址与端口，失焦或回车即时生效，无需「保存」
- 「检查连接」对设备 SSH 端口做一次 TCP 握手，显示往返延迟或具体失败原因
- 配置存在 `~/Library/Application Support/com.zoe.rmbin/settings.json`，先写临时文件再 rename
- USB 直连时地址固定 10.11.99.1；Wi-Fi 下在设备「设置 › 通用 › 关于本机 › 版权与许可」查看

## 下一步（TODO）

- reMarkable 设备自动发现（mDNS）
- 照片（连续色调）描出来是一团麻，目前只有线稿好看——需要先做减色/边缘提取
- 画在「当前打开的那一页」上，落笔位置和页面已有内容无关，会盖上去
