# Swoop Privacy Statement

**Last updated: October 16, 2026**

Swoop ("the software") is an open-source download manager developed by the Swoop open-source project. This statement describes what personal data Swoop processes, and how. It is written in the spirit of privacy statements such as the [Microsoft Privacy Statement](https://www.microsoft.com/zh-cn/privacy/privacystatement), adapted to a small desktop application.

中文版见下文（Chinese version below）。

---

## English

### 1. Overview

Swoop does not operate any servers, accounts, advertising, or analytics. The software is designed so that **no personal data is ever transmitted to the developer or to any third party by Swoop itself**.

### 2. Personal data we collect

**We do not collect any personal data.** Swoop contains no telemetry, no crash reporting, no usage analytics, no advertising SDKs, and no trackers of any kind.

### 3. Data stored locally on your device

To perform its functions, Swoop stores data **locally on your device only**, under your user profile:

- Download task information (URLs, progress, queue and schedule settings)
- Application preferences (speed limits, categories, network and proxy settings)
- Site credentials, **only if you explicitly save them** — these are stored in the operating system's credential store (Windows Credential Manager / macOS Keychain), not by Swoop's own files
- Site-grabber projects you create

This data never leaves your device except when you yourself export or copy it.

### 4. Network activity

Swoop is a network application, so it necessarily communicates with machines **you** point it at:

- **Downloads**: Swoop connects to the URLs you provide (directly or via the proxy settings you configure). The remote server sees a standard HTTP/FTP request; Swoop sends no identifying metadata beyond what the protocol requires.
- **Software updates**: Swoop periodically checks the update source you configured (default: the project's GitHub Releases) for new versions and verifies signatures with a built-in public key. This request contains no personal data.
- **Browser extension**: The optional Chromium/Firefox extension communicates with the Swoop app on your own machine (localhost) and reads page URLs only to hand downloads to Swoop. It performs passive media detection locally and does not report anything to anyone.

### 5. How we use data

Since we receive no data from the software, there is nothing to use. The developer has no access to your downloads, settings, or usage.

### 6. Sharing personal data with third parties

We do not receive, and therefore do not sell, rent, or share any personal data with third parties.

Note: when you download from a website, that website's own privacy policy governs what that site collects — Swoop has no control over remote servers.

### 7. Cookies and tracking technologies

Swoop does not set cookies and does not use any tracking technologies. (When mirroring websites, the site grabber honors `robots.txt` and sends only ordinary HTTP requests.)

### 8. Security

- Updates are signed (Ed25519) and verified before installation.
- Unfinished downloads are written through temporary files; no third party can inject content into your downloads beyond what the source server serves.
- Credentials are delegated to the OS credential store, protected by your OS account.

### 9. Children's privacy

Swoop is not directed at children and collects no personal data from anyone, including children.

### 10. Changes to this statement

If the software's behavior changes in a way that affects privacy, this statement will be updated in the project repository before the release ships.

### 11. Contact

Privacy questions: open an issue at https://github.com/i-mufeng/muffin-swoop or contact the maintainer via GitHub.

---

## 简体中文

### 1. 概述

Swoop 是由开源项目开发的下载管理器。本软件**不架设任何服务器、不提供服务账号、不含广告与分析工具**，设计上保证软件本身不会向开发者或任何第三方传输你的个人数据。

### 2. 我们收集的个人数据

**我们不收集任何个人数据。** Swoop 不包含遥测、崩溃上报、使用统计、广告 SDK 或任何形式的追踪器。

### 3. 存储在你设备上的本地数据

为完成其功能，Swoop 仅在**你设备本地的用户目录下**存储数据：

- 下载任务信息（URL、进度、队列与计划设置）
- 应用偏好设置（限速、分类、网络与代理设置）
- 站点凭据——**仅在你明确选择保存时**，且存储于操作系统凭据管理器（Windows 凭据管理器 / macOS 钥匙串），不由 Swoop 自行存储
- 你创建的网站抓取项目

除非你自己导出或复制，这些数据不会离开你的设备。

### 4. 网络活动

Swoop 是网络应用程序，因此必然会与你指定的目标服务器通信：

- **下载**：Swoop 连接你提供的 URL（直接或通过你配置的代理）。远程服务器收到的是标准 HTTP/FTP 请求，除协议必需内容外不附带任何识别性元数据。
- **软件更新**：Swoop 会按你配置的更新源（默认：项目的 GitHub Releases）定期检查新版本，并用内置公钥验证签名。该请求不包含任何个人数据。
- **浏览器扩展**：可选的 Chromium/Firefox 扩展仅与本机上的 Swoop 应用（localhost）通信，读取页面 URL 用于接管下载；媒体嗅探在本地完成，不向任何一方上报。

### 5. 我们如何使用数据

由于我们不从软件接收任何数据，因此不存在数据使用行为。开发者无法访问你的下载内容、设置或使用情况。

### 6. 与第三方共享

我们不接收任何个人数据，因此不会出售、出租或与第三方共享个人数据。

请注意：当你从某个网站下载内容时，该网站自身的隐私政策适用于该网站收集的数据——Swoop 无法约束远程服务器。

### 7. Cookie 与追踪技术

Swoop 不设置 Cookie，不使用任何追踪技术。（网站抓取功能遵守目标站点的 `robots.txt`，仅发送普通 HTTP 请求。）

### 8. 安全性

- 更新包经 Ed25519 签名，安装前验证。
- 未完成的数据经临时文件写入；除源服务器提供的内容外，第三方无法向你的下载中注入内容。
- 凭据交由操作系统凭据管理器保管，受你的系统账户保护。

### 9. 儿童隐私

Swoop 不面向儿童，也不收集任何人的个人数据（包括儿童）。

### 10. 本声明的变更

如软件行为发生影响隐私的变化，将在发布前于项目仓库中更新本声明。

### 11. 联系方式

隐私问题请通过 https://github.com/i-mufeng/muffin-swoop 提交 issue 或通过 GitHub 联系维护者。

---

*This project is open source under the MIT License: https://github.com/i-mufeng/muffin-swoop*
