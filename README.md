# RM Bin

**拖一张图进去，它出现在你的 reMarkable 上。**

一枚停在桌角的迷你 Paper Pro。松手的那一刻，图片被包成一份 PDF，秒级送到设备的文档库里——原图，能用笔批注。

<img src="docs/img/hero.jpg" width="640" alt="RM Bin 悬停在桌面上" />

<sub>静止时它是半透明的，不打扰任何东西。有文件靠近才会亮起来、抬起来。</sub>

### [↓ 下载 for macOS](https://github.com/JIACHENG135/rm-bin/releases/latest)

已用 Developer ID 签名并通过 Apple 公证，首次打开不会被拦 · [网站](https://jiacheng135.github.io/rm-bin/)

---

## 它解决什么

把一张图弄到 reMarkable 上，官方的路子是：打开桌面端、导入、等同步、在设备上翻出来。RM Bin 把它压缩成一个动作——**拖过去，松手**。

图片被包成一页 PDF，POST 给设备自己的导入接口。不描线、不二次编码、不用停设备界面——原图原样进设备的文档库，还能直接用笔在上面写字。

## 怎么用

| | |
|---|---|
| **拖** | 图片拖到那台小设备上。纸面抬起来，跟着光标磁性倾斜。不是图片？它轻轻摇头。 |
| **传** | 图片包成单页 PDF，交给设备自己的 USB 网页导入接口。 |
| **落地** | 完成时 Marker 手写一个对勾；连不上设备就摇头，不骗你。 |

## 设置

`⌘,`，或者右键那台小设备——窗口上不放任何按钮，右键弹出的是真正的系统菜单。

<img src="docs/img/settings.jpg" width="520" alt="设置窗口" />

- 改完失焦即生效，没有「保存 / 取消」
- 「检查连接」对设备 SSH 端口做一次真实握手，报往返延迟，或者到底哪一步不通（超时 / 拒绝 / 解析失败，分得清清楚楚）
- 背景是 macOS 26 的 Liquid Glass（`NSGlassEffectView`），旧系统自动退回经典材质，不会变成一块灰板
- 配置存在 `~/Library/Application Support/com.zoe.rmbin/settings.json`，先写临时文件再 rename

## 装它

下载 [最新版 `.dmg`](https://github.com/JIACHENG135/rm-bin/releases/latest)，拖进「应用程序」。

设备上需要打开「设置 › 通用 › 存储 › USB 网页界面」，并用 USB 线连接——默认地址固定是 `10.11.99.1`。走 Wi-Fi 时地址在 **设置 › 通用 › 关于本机 › 版权与许可** 里能查到，但 USB 网页界面默认只绑在 USB 网卡上；这种情况下 RM Bin 会退回走 ssh 直接把文件写进设备的文档库（需要 Mac 能免密 ssh 到 `root@设备地址`，`ssh-copy-id` 一次即可）。

**需要**

- macOS 11+（Liquid Glass 效果需要 macOS 26）
- Apple Silicon
- reMarkable 2 / Paper Pro，USB 直连，或与电脑同网络（走 ssh 回退）

## 做成什么样

- **窗口上没有任何控件。** 设置和退出都在右键菜单里，由系统绘制。悬停时唯一的反馈是 Marker 往外滑 1.5px——像拿起设备时笔会错位一点
- **静止时缓慢呼吸**，偶尔在屏角画一笔涂鸦，然后自己淡去
- **失败也不骗你**：设备连不上，就不写对勾，改成摇头

## 自己构建

```sh
npm install
npm run tauri dev     # 开发
npm run release       # 构建 + 签名 + 公证 + 校验（需要 Developer ID）
```

需要 Rust 和 Node。**不要用 `sudo`**——它会把构建产物写成 root 所有，还会让 codesign 去读 root 的钥匙串，报出一个和签名毫无关系的 `errSecInternalComponent`。

---

# 它怎么做到的

RM Bin 把图片包成一份单页 PDF（[`rm/pdf.rs`](src-tauri/src/rm/pdf.rs)），POST 给设备自己收文档用的 HTTP 接口。PDF 是手写的——五个对象加一张交叉引用表，用不着为此引一个几千行的库；图片以 `/DCTDecode` 原样嵌入，不二次编码，所以设备上看到的就是原图，色彩空间也不变。

首选路径是 USB 网页界面：不用 SSH、不用密钥、不用停设备界面，平板本来就有一个专门收文档的接口。它不可达时（比如走 Wi-Fi，网页界面默认只绑 USB 网卡），退回走 ssh：把 `.metadata`/`.content`/`.pdf` 三个文件直接写进 xochitl 的文档库（[`rm/upload.rs`](src-tauri/src/rm/upload.rs)），写完重启 xochitl 让它读到。两条路共用同一个「一次 ssh 连接、按精确字节数分帧、全部写完才重启」的脚本生成器，因为需要小心的地方是一样的：`dd bs=N count=1 iflag=fullblock`，不是 `head -c N`——所有文件顺着一条 stdin 下来，每条命令必须精确吃掉自己的那一份，busybox 的 `head -c` 按块读会吃过界。

## 下一步（TODO）

- reMarkable 设备自动发现（mDNS）
- Wi-Fi 下 USB 网页界面不可达时的 ssh 回退已经在，但还没有自动探测走哪条路更快——目前是先试网页接口，超时了才退回 ssh
