<!-- SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT -->
<!-- SPDX-FileCopyrightText: 2026 movblock -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# PhantomLoaderQQNT

[![License: LGPL-3.0-or-later](https://img.shields.io/badge/License-LGPL--3.0--or--later-blue.svg)](LICENSE)
[![GitHub release](https://img.shields.io/github/v/release/mlbv/PhantomLoaderQQNT?logo=github)](https://github.com/mlbv/PhantomLoaderQQNT/releases)
[![Discord](https://img.shields.io/badge/Discord-加入社区-5865F2?logo=github&logoColor=white)](https://discord.gg/8nBnd3dTG)

一个基于 LiteLoaderQQNT 的 QQNT 插件平台。

**版本 1.0.0 · LiteLoader API 兼容版本 5.2.1 · Windows 11 x64 · QQNT 9.9.37-54395**

## 安装与启动

从 [Releases](https://github.com/mlbv/PhantomLoaderQQNT/releases) 下载完整的 Windows x64 离线发行包，核对 SHA-256 后解压到 QQ 安装目录以外的新目录。

1. 关闭 QQ。
2. 运行 `Install.cmd`，或在 PowerShell 中执行 `.\Install.ps1`。写入 Program Files 下的 QQ 安装通常需要管理员权限。
3. 使用桌面上的 **QQNT (PhantomLoader)** 快捷方式启动。

默认运行目录为 QQ 安装目录下的 `PhantomLoaderQQNT`，插件和数据位于 `%LOCALAPPDATA%\PhantomLoaderQQNT\profile`。支持通过安装参数指定其他位置。卸载时运行发行包或已安装目录中的 `Uninstall.cmd`；插件与数据保留。

完整参数、校验与卸载说明见 [Windows distribution](docs/distribution.md)。发行包由 GitHub Actions 从 PGP 签名的标签与提交构建，同时提供匹配源码、SHA-256 和构建来源证明；校验步骤见 [Release verification](docs/releases.md)。

## 插件与兼容性

在设置中的 **PhantomLoaderQQNT** 页面安装、启用或移除插件，也可以将插件目录放入当前 profile 的 `plugins` 目录。

插件通过 manifest 的 `"api": "PhantomLoader"` 选择扩展 API，并在 `onLoad(api)` 中接收。默认 API 为 LiteLoader 5.2.1，原有插件继续使用 `LiteLoader` 全局对象；两个 API 共享插件注册表与配置。

~~~js
exports.onLoad = (PhantomLoader) => {
    PhantomLoader.versions.phantomloader; // "1.0.0"
    PhantomLoader.api.intercept.register("my-plugin", "external", handler);
};
~~~

插件清单继续支持 LiteLoader manifest v4。兼容 API 的范围与配置路径见 [Compatibility](docs/compatibility.md)。

`PHANTOMLOADERQQNT_PROFILE` 指定 profile；旧的 `LITELOADERQQNT_PROFILE` 仍可使用，新变量优先。

## 开发

- [Interception API](docs/interception.md)：主进程消息与外部链接拦截。
- [Native API](docs/native.md)：主进程原生地址、调用与钩子。
- [Native runtime architecture](native/README.md)：启动器、payload 与原生钩子的内部实现及构建方式。
- [Distribution build](docs/distribution.md#building-the-bundle)：构建完整离线发行包。
- [QQNT findings](docs/qqnt.md)：QQNT 遥测、安全报告与 DLVM 功能。

## 安全

手动映射的 payload 不出现在 `K32EnumProcessModules` 等常规模块枚举结果中，但仍可通过内存查询、代码完整性、函数表、调用栈、线程、VEH、PEB、IPC 和句柄检查被发现；具体限制见[原生运行时说明](native/README.md#inspection-limits)。每次 QQNT 更新后均须重新分析二进制，以检查是否新增或改变了用户态检测机制。

### 遥测

我们强烈建议自行配置hosts阻断腾讯遥测域名。

### 虚拟机

QQNT 包含大量的虚拟机检测代码，目前 PhantomLoader 并没有做相关绕过。如必要，请尽量将你的虚拟机伪装的像真机一点。

### DLVM 拦截和样本捕获

QQNT 允许服务端**动态下发并静默执行*任意*程序**。为避免未知的探测代码在 PhantomLoader 加载后运行，DLVM 捕获默认启用，并在第三方插件加载前拦截匹配的服务器下发程序。发现任务后，系统会先将程序字节与元数据写入 profile 的 `data/dlvm_capture/captures`，再终止 QQ 主进程，以阻止该回调继续执行。

通过 PhantomLoader 快捷方式启动时，独立启动器会在 QQ 退出后显示报告窗口。
> [!NOTE]
> PhantomLoader仅会在本地打包样本和元数据并提供 [本仓库的 GitHub Issue](https://github.com/mlbv/PhantomLoaderQQNT/issues) 入口；样本本身须手动上传

设置中的 **DLVM 程序捕获** 可关闭此功能，重启后生效；也可以在 profile 的 `data/LiteLoader/config.json` 中设置 `"enable_dlvm_capture": false`。详细行为见 [发行说明](docs/distribution.md#default-program-capture)。

## 许可证

代码及文档中的代码示例采用 [GNU LGPL 3.0 或后续版本](./LICENSE)，文档采用 [Creative Commons Attribution 4.0 International（CC BY 4.0）](./LICENSES/CC-BY-4.0.txt)。许可文本见 [LICENSES](./LICENSES)，第三方许可见 [native/licenses](./native/licenses)。

## 感谢

PhantomLoaderQQNT 和 LiteLoaderQQNT 所有开发者和贡献者的提交。

- [LiteLoaderQQNT 原版仓库](https://github.com/LiteLoaderQQNT/LiteLoaderQQNT)
- [MrNya907/LiteLoaderQQNT 前置分支](https://github.com/MrNya907/LiteLoaderQQNT)
