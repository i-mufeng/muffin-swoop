# swoop-grabber

UI 无关的 HTTP(S) 站点抓取库。按用户创建的项目抓取公开页面，默认严格限制在起始 URL 的同一个 origin（包含协议和端口），输出可恢复的原始对象与离线镜像。

## 接入

```rust,no_run
use swoop_grabber::{Grabber, GrabberConfig};
use tokio_util::sync::CancellationToken;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let config = GrabberConfig {
    start_url: "https://example.com/".into(),
    output_dir: "capture/example".into(),
    max_depth: 2,
    max_pages: 200,
    ..GrabberConfig::default()
};
// create/open 会读写项目文件，应放应用后台线程；不从 GUI 绘制函数调用。
let mut grabber = Grabber::create(config)?;
let updates = grabber.subscribe(); // watch 接收完整只读快照
let cancel = CancellationToken::new();
let result = grabber.run(cancel).await?;
println!("下载 {}，失败 {}", result.downloaded(), result.failed());
// 以后使用 Grabber::open("capture/example") 恢复；成功对象校验后复用。
# Ok(()) }
```

`snapshot()` 返回 `ProjectSnapshot`；`subscribe()` 可在独立任务中等待状态变化。`run` 的 `Complete` 表示没有待处理条目，调用方仍必须显示 `failed()` 和每项 `PageState`，不能把队列耗尽当成全部成功。取消返回 `Cancelled`，字节/数量限制返回 `Limited`；待处理条目仍可恢复。提高预算/深度/页数等可调用 `update_config`，不能改起始网址与项目目录。

## 默认配置和边界

| 配置 | 默认 | 约束与含义 |
| --- | --- | --- |
| `max_depth` | 2 | 0–100；普通页面链接和 iframe 加一层，静态资源及递归 CSS 保持当前层 |
| `max_pages` | 200 | 1–100000；包含 HTML 和资源，所有项目条目合计上限 |
| `max_total_bytes` | 100 MiB | 累计接纳到预算内的响应体 payload，重启不归零，包括 robots 与失败/取消前接纳的块 |
| `concurrency` | 4 | 1–16；同层有界并发，循环 URL 只入队一次 |
| `allowed_domains` | 空 | 额外精确域名；不隐式允许子域名，不接受通配符或用户名密码 |
| `resource_extensions` | 空 | 空为不限；非空对 HTML/CSS 静态资源按 URL 扩展名过滤，页面导航不受扩展名过滤 |
| `respect_robots` | true | 检查 robots Allow/Disallow，使用 `SwoopGrabber` UA |
| `max_file_bytes` | 16 MiB | 1 字节–256 MiB；单响应内存及文件预算 |
| `request_timeout_secs` | 20 | 1–120 秒；连接/响应总超时，并支持取消 |

全局字节预算通过原子预留协调并发；不会把超过预算的块写入项目。HTTP/TCP 层可能预读数据，因此该值不是网卡流量的精确计费上限。所有项目状态 JSON 限制 128 MiB；默认小型项目适合此实现，大型项目的全快照持久化存在额外 CPU/磁盘成本。镜像会保留独立代次，磁盘空间不只等于响应体预算。

## 抓取与镜像行为

- 只发送 GET；不发送 Cookie / Authorization、不提交表单，不接受 URL 用户名/密码。默认不执行 JavaScript；可显式启用隔离 Chromium 动态渲染，或导入已渲染 DOM 快照，详见 [站点抓取配置](../../docs/grabber.md)。
- 规范化相对 URL、HTML `<base>`、片段去重；保留查询参数与顺序。每次重定向重新检查允许域及 robots，最多跟随 5 次。
- 使用 HTML 解析器处理 `a/area[href]`、stylesheet/icon/preload 的 `link[href]`、`img/script/source/audio/video/embed[src]`、`iframe/frame[src]`、`video[poster]`、`object[data]`，并解析 `img/source[srcset]` 的 URL/密度/宽度候选；不会把 data URL 内部的逗号误拆成下载链接，无效描述符候选保持原样。
- 使用 CSS Syntax 解析器发现并重写外部 CSS、`style` 属性和 `<style>` 元素里的 `url()` 与 `@import`，递归导入和导入环复用同一个去重队列。注释与普通 CSS 字符串不产生下载请求；保留 data / blob URL 及 CSS 的 `url(#local)`。CSS 语法嵌套解析限制 64 层。CSS 按 `text/css` 识别，`application/octet-stream` 且 URL 为 `.css` 时也识别；未声明类型的其他后缀不猜测为 CSS。
- HTML/CSS 支持 BOM、HTTP charset、HTML 前 1024 字节中的 meta charset / http-equiv、CSS 开头 `@charset` 的显式字符集声明，使用 `encoding_rs` 解码（包括 GBK、Windows-1252）。无声明默认 UTF-8，不猜测本地语言编码；未知声明或无效编码明确报失败。记录实际编码供恢复使用，原始对象保留原字节；镜像输出 UTF-8 BOM，使旧 meta / `@charset` 不会误解码新字节。未实现 CSS 从父页面继承字符集或浏览器的全部启发式编码检测。
- 默认静态模式不处理 JavaScript 生成的请求；未实现 Service Worker 离线缓存、登录流程、其他运行时派生资源（配置动态渲染可补获 DOM 中的生成节点）；这些网站可能无法完全离线使用。下载脚本文件不会在抓取阶段执行它；打开镜像后浏览器自身的运行行为不由抓取器控制。
- 使用 [Google robots 解析器的 Rust 移植](https://docs.rs/robotstxt/latest/robotstxt/)检查 Allow/Disallow。404/410 视为未提供规则；认证错误、5xx、网络失败、robots 重定向均拒绝继续该 origin，项目里记录原因。未实现 Crawl-delay、Sitemap 扩展发现或浏览器挑战页处理；可在用户配置中显式关闭 robots 检查。
- URL 与内容分别做 SHA-256；对象及镜像文件名只含哈希与安全扩展名，不使用网页提供的文件路径，查询变体不会覆盖彼此。
- 成功下载的引用重写成镜像内相对文件名并保留片段；未下载、失败、超界引用保留其线上绝对 URL。移除 `<base>`，防止镜像相对链接又被指向线上站点。离线镜像不等于完全无网络引用。

## 文件与恢复

输出目录可有其他用户文件，这些文件不会被覆盖。项目私有目录 `.swoop-grabber/` 必须由 `create` 新建；已有项目必须调用 `open`。所有对象、状态快照和每代 `mirror-*` 文件都用新建模式写入；已有同名对象只有逐字节相同才复用，否则返回错误。每代镜像独立，旧镜像保留。

`open` 从最新完整状态快照恢复，跳过未写完的快照，并验证原始对象路径、文件类型、大小和 SHA-256；不跟随对象目录/文件符号链接。失败或未完成项下次 `run` 重试；已成功页面的原始 HTML/CSS 可重新生成待抓链接，支持提高限制后继续，不重新下载成功资源。损坏对象不会被悄悄覆盖。`run` 与 `update_config` 使用跨进程非阻塞独占文件锁，同一项目被其他实例使用时明确返回错误；进程退出后操作系统自动释放锁。

## 验证

`cargo test -p swoop-grabber` 使用真实本地 TCP/HTTP 服务器，覆盖循环/深度、相对链接与 `<base>`、查询参数、资源过滤、robots、跨 origin 重定向、并发与字节上限、取消、重启、镜像引用和旧文件保留；还覆盖递归 CSS 导入环、带查询/片段的字体图片、失败资源绝对 URL 回退、data/blob 保留、srcset 逗号与无效候选、内联样式、声明编码和 CSS 恢复不重复下载。解析单元测试另覆盖 CSS 转义/注释/深层嵌套和字符集错误。不把夹具通过视为任意线上站点或 Windows 实机已验收。

扩展资源、URL 排除规则、脚本镜像选项、浏览器渲染和手动 DOM 导入的完整说明见 [docs/grabber.md](../../docs/grabber.md)。自动浏览器默认关闭，独立 profile、单独预算，并保留 Windows 实机验收。
