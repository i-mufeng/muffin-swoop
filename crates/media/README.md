# swoop-media

独立的 HLS/DASH 清单检查、MMS 录制与 FFmpeg 转封装库。支持 HTTP(S) HLS 主/子清单、普通 AES-128 HLS、无 DRM 的 DASH；HLS 与 DASH 的点播及限时直播共用会话认证、受控代理网关和请求重试。可选择清晰度、独立音轨及已支持的文本字幕，输出 MP4/MKV，音视频不重新编码；MMS 使用独立的 MMSH/MMST 输入并输出限时 MKV。可配置强 ETag 校验的完整分片恢复缓存。应用提供异步检查、持久化选择、媒体任务历史和默认偏好。已验证的来源组合、FFmpeg 兼容性及 Windows 实机边界见各节；不将协议入口等同于所有服务器兼容。

## 接入

根 workspace 加入 `crates/media`，需要使用的 crate 添加 `swoop-media = { path = "../media" }`。Tokio 运行时需支持进程、文件和异步 IO；本库 manifest 已补 `process` / `net` features。FFmpeg 单独安装，可配置绝对路径；不会运行 shell、安装工具或提升系统权限。

```rust,ignore
use swoop_media::{MediaConfig, MediaDownloader, MediaRequest, MediaSelection, MediaSource};
use tokio_util::sync::CancellationToken;

let downloader = MediaDownloader::new(MediaConfig {
    ffmpeg_path: "/opt/homebrew/bin/ffmpeg".into(), // Windows: C:\\tools\\ffmpeg.exe
    max_concurrent: 2,
    ..Default::default()
})?;
let cancel = CancellationToken::new();
let source = MediaSource::new("https://example.org/video/master.m3u8");
let info = downloader.inspect(&source, &cancel).await?;
let result = downloader.download(
    MediaRequest {
        source,
        destination: "下载/视频.mp4".into(), // 父目录须已存在
        duration_limit: None, // 直播必须指定 Some(Duration)
        selection: MediaSelection::default(), // 自动最佳；也可按高度、候选 ID、音轨/字幕选择
        overwrite: false, // 只有本次明确授权时才传 true
    },
    cancel.clone(),
    |event| { /* 快速转发至 UI；不要阻塞或 panic */ },
).await?;
```

`MediaSource::validate()` 可在桥接接收前同步检查 URL 与头部，无网络；`MediaDownloader::capabilities(&CancellationToken)` 查询配置的 FFmpeg 是否支持 HLS、DASH、MMSH/MMST、ASF、文本字幕编解码器和默认 TLS 验证。能力结果按 downloader 缓存；缺少所需能力明确失败，不回退普通文件下载。

## 清晰度与音轨

`MediaInfo.variants` 给出 HLS `EXT-X-STREAM-INF` 或 DASH 视频 Representation 的分辨率、带宽、codec 与不透明稳定 ID；`audio_tracks` 给出可选独立音轨名称、语言、带宽及 codec（仅展示清单实际声明的值）。DASH 支持继承 AdaptationSet 上的属性；未提供可辨识媒体类型或稳定 Representation id 的条目无法可靠手选。`audio_tracks_for(&VideoSelection)` 返回与当前视频兼容的音轨，HLS 会约束 AUDIO rendition group。

`MediaSelection` 可序列化，包含：

- `video: Best`（默认）：按清单声明的像素数、带宽选最佳；`MaxHeight(n)` 在不超过高度的候选内选最佳；`Variant(id)` 精确匹配。没有清单候选的普通播放列表使用 FFmpeg 自动选择。高度未知或没有符合上限的条目明确失败，不悄悄下载更高清晰度。
- `audio: Auto`（默认）：兼容组中优先默认音轨、带宽；`PreferredLanguage(tag)` 优先大小写不敏感的精确语言标签，没有匹配时回退自动；`Track(id)` 精确音轨；`None` 不输出音频。

HLS 指定质量使用检查过的子清单，独立音轨采用另一个同样受协议、TLS、超时、头部限制的输入。DASH 按 FFmpeg 流的 Representation `id` 元数据精确映射，重复 ID 或包含无法无歧义表达的流选择符字符会报 `InvalidSelection`；FFmpeg 探测阶段仍可能读取其他 Representation 的初始化数据。下载启动时重新检查来源，已删除或不兼容的选择明确失败。稳定 ID 是来源标识的 SHA-256，不暴露含 token 的子清单 URL。

应用保存默认高度/优先语言，以及每个任务的完整选择；重启后的从头重试继续使用相同选择。媒体窗口的“检查清晰度”运行在后台，可取消；切换草稿不会应用旧检查结果，后续浏览器链接按 FIFO 保留。多语言混入同一子清单但没有独立 rendition URI 的 HLS 音轨、DASH 多 Period 连续拼接不在本轮验收范围。

## 字幕选择

`MediaInfo.subtitle_tracks` 展示字幕名称、语言、清单编码和支持状态；`subtitle_tracks_for(&VideoSelection)` 按 HLS `SUBTITLES` 组过滤。`MediaSelection.subtitle` 为 `SubtitleSelection::None`（默认关闭）或 `Track(id)`（精确一条）；旧历史未包含该字段时仍为关闭，选择随任务历史保存，重启重试保留。界面检查异步执行，新增字幕菜单不改变浏览器 FIFO 草稿行为。

当前验证支持点播 HLS 的独立 WebVTT rendition（包括多个 `.vtt` 分片与 `X-TIMESTAMP-MAP`）、MPEG-TS HLS 限时直播 WebVTT，以及 DASH `text/vtt`、SRT（`application/x-subrip` / `text/srt`）、ASS/SSA（`text/x-ass` / `text/x-ssa`）和声明 `contentType="text"` / `codecs="tx3g"` 的 mov_text Representation。HLS 字幕 `NAME` 须在清单内唯一并可作为 FFmpeg 元数据选择符；DASH ID 须唯一。未支持的 TTML/IMSC、封装 wvtt 和图形字幕显示为不可选；后端同样拒绝，不能只改持久化字段绕过。直播字幕不再一概禁止，但实际组合仍受 FFmpeg 能力限制：本机 FFmpeg 9.0.1 的 fMP4 HLS 直播字幕夹具产生重复 MOOV 诊断，严格失败且不发布；不能把已通过的 MPEG-TS 组合扩大为所有直播封装验收。

MP4 文本字幕编码为 mov_text，MKV 编码为 SubRip/SRT；语言按容器能力保存（MP4 要求三字母 ISO 639 标签；清单只有两字母或地区标签时，播放器可能显示未指定），复杂字体、排版和样式不承诺保留。FFmpeg 须具备对应字幕 decoder/demuxer，以及 mov_text（MP4）或 SubRip（MKV）encoder；缺失能力返回明确错误。字幕采用独立且受同样协议/头部/TLS 限制的清单输入，避免 FFmpeg 通用探测跳过 HLS 开头字幕、重复 DASH 字幕。音视频正常探测；字幕输入关闭未使用的音视频及其声道猜测。输出显式映射一条字幕，并启用 `empty_output_stream` 失败保护；空字幕、缺失后续字幕片、失效选择不会触发完成。没有降低诊断级别或去掉 `-xerror`。

参考 [FFmpeg HLS 格式文档](https://ffmpeg.org/ffmpeg-formats.html#hls-2) 与 [FFmpeg 流选择和输入参数](https://ffmpeg.org/ffmpeg-doc.html)。本机实际工具与产物验证见下文；不以清单声明或命令参数作为支持证据。

`MediaDownloader` 的克隆共享并发额度。等待额度、检查清单、运行 FFmpeg 都响应 `CancellationToken`。调用方取消后必须继续 await 下载 future，才能确认子进程已结束、临时文件已删除；不要把直接丢弃 future 当作完整取消协议。

`MediaEvent` 包含 `Waiting`、`Started { info }`、`Progress { output_bytes, media_time, speed }` 和 `Completed { bytes }`。进度来自 FFmpeg 的 `-progress`，不是网络传输字节；转封装时间与网络速度不能混作普通文件下载速度。`MediaResult` 返回目标路径、文件大小和检查信息。错误不会带请求 URL、请求头值或 FFmpeg stderr，带 token 的源 URL 不应另行写入调用方日志。

输出先写入规范父目录的唯一 `.swoop-media-*.part`；成功并同步文件后才发布。`MediaRequest.overwrite=false` 保持 no-clobber，已有目标返回 `DestinationExists`。`true` 是单次明确授权：开始时读取已有普通文件的规范路径、SHA-256 与 metadata；最终在后台再次校验，内容、时间/标识或父目录变化会拒绝覆盖。所有大文件 hash 都运行在 blocking worker，每 64 KiB 检查取消。符号链接/目录返回 `UnsafeDestination`；准备阶段不存在的目标仍使用 no-clobber，不覆盖后来出现的文件。

覆盖先将实际目标移动到同目录唯一 `.swoop-media-recovery-*.bak`，再验证被移动文件的内容、身份及写入时间（仅忽略移动本身造成的 ctime 变化），最后以 no-clobber 发布完成文件；不会采用“检查后无条件覆盖”的方式。这是两个发布步骤，目标路径可能短暂缺失。若外部程序抢先保存了同名文件，它不会被新产物或回滚覆盖。校验失败或提交前取消时尝试 no-clobber 恢复；若恢复受阻，返回 `RecoveryRequired(PathBuf)` 并保留备份，调用方须显示恢复路径，即使取消令牌已经触发也不能吞掉此错误。恢复文件不受临时文件自动清理控制。

下载后字节完全相同则保留原文件及 inode，并正常发出 `Completed`。提交成功后才清理仍符合已核验身份及写入 metadata 的备份；提交后才到达的取消仍按完成处理；清理前观察到取消时保留备份。覆盖授权不应持久化，重新开始须由应用当前策略重新决策。普通下载失败清理本次产物临时文件；突然退出可能留下恢复文件或短暂缺失的目标，媒体库不会在重启时自动删除或覆盖恢复文件，须检查备份后手动恢复。外部移动整个父目录仍不承诺零残留，旧打开句柄的后续写入不受跨进程锁控制。Windows 原生移动、发布和恢复行为需另行实机验收。

## 请求限制

- HLS/DASH 清单与子资源只接受 HTTP(S)；MMS 根输入另接受 `mms://`、`mmsh://`、`mmst://`。均禁止 URL 内嵌用户名和密码及控制字符，MMS 根输入还禁止片段。
- 请求头只允许 `User-Agent`、`Accept`、`Accept-Language`、`Referer`；Referer 被裁剪至 origin，移除路径、查询及片段。所有头禁止 CR/LF/NUL。
- `MediaSource.headers` 中的 `Cookie`、`Authorization`、`Proxy-Authorization`、任意自定义头继续直接报 `UnsupportedHeader`。HLS/DASH 会话认证仅通过下文的结构化 `MediaSessionAuth`，从不写入 FFmpeg 全局头或任务历史。浏览器原始 Cookie 缺少作用域元数据，仍拒绝接收。
- HLS 递归检查子清单，默认每份最多 2 MiB、最多 32 份、最多 4 层。可配置字节与数量预算；URL、key/map/segment 引用检查也在解析阶段执行。
- 普通 HLS `METHOD=AES-128` 支持 HTTP(S) key URL；`SAMPLE-AES`、非 identity KEYFORMAT、DASH `ContentProtection` 拒绝。不调用许可证接口，不提供 DRM 解密或绕过。
- HLS/DASH 的 FFmpeg 输入协议白名单固定为 `http,https,tcp,tls,crypto`；独立 MMS 输入为 `mmsh,mmst,http,https,tcp,tls`，不提供关闭限制的配置；即使远程清单在检查后改变，`file`、`pipe`、`concat` 等本地输入协议仍被拒绝。输出文件是库选定的独立输出参数。
- 旧 `MediaDownloader::new` 默认直连：清单检查与 FFmpeg 不继承代理环境变量；需要代理时显式使用 `new_with_network`。代理错误不会重试为直连；仅显式排除规则绕过代理，不修改系统代理。
- FFmpeg 不继承 `FFREPORT`，stderr 只用固定大小缓冲检查有无 warning/error，不保留、不展示 URL/头部；只要有诊断便不定稿，防止 FFmpeg 跳过缺失分片后以退出 0 生成残缺视频。该策略可能拒绝带良性警告的媒体，优先避免错误标记完成；下载失败返回退出码或通用诊断错误。下载前要求所用 FFmpeg 的 TLS 协议默认开启证书验证，HTTPS 根输入还显式启用验证；不兼容的旧构建返回明确能力错误。

## HLS/DASH 会话认证、代理与直播

推荐仅将本次明确输入的凭据放入 `MediaSessionAuth`：支持 Basic、Bearer，以及明确指定 path / Secure / 到期时间的 Cookie。`OriginCredentials` 以完整 origin（协议、主机、有效端口）为边界；不做子域授权。Cookie 还按路径目录边界、Secure 和过期时间筛选，不解析浏览器原始 Cookie 来猜测作用域。所有认证类型不实现 serde，Debug 固定脱敏；应用仅在内存保留登录，历史仅保存 `auth_required` 标记。更改地址、载入新草稿或关窗清除表单凭据；表单额外绑定 origin 防止复用。退出后，未完成的私有任务需重新输入，后台/CLI 不会匿名恢复。

```rust,ignore
use swoop_media::{MediaAuthorization, MediaNetworkConfig, MediaProxyConfig,
    MediaSessionAuth, OriginCredentials};
let downloader = MediaDownloader::new_with_network(config, MediaNetworkConfig {
    proxy: Some(MediaProxyConfig {
        url: "socks5h://127.0.0.1:1080".into(),
        bypass: vec!["localhost".into(), "127.0.0.1".into()],
    }),
    ..Default::default()
})?;
let auth = MediaSessionAuth::new(vec![OriginCredentials::new(
    "https://media.example.org",
    Some(MediaAuthorization::Bearer(user_entered_token)),
    vec![],
)?])?;
let info = downloader.inspect_with_auth(&source, &auth, &cancel).await?;
let result = downloader.download_with_auth(request, auth, cancel, on_event).await?;
```

`new_with_network` 支持手动 HTTP / HTTPS / SOCKS5 / SOCKS5h 代理和排除规则；代理 URL 禁止内嵌账号密码。默认 `use_system_proxy=false`；只有明确选择时才使用系统代理。可配置连接/读取超时、重定向上限、资源数和并发请求数；默认分别 15 秒 / 30 秒 / 8 跳 / 20,000 个 / 16 个。`request_retries` 默认 2（允许 0–10），`retry_delay_ms` 默认 250（0–30,000）；只对响应正文开始前的连接/超时、408、429、5xx 做指数退避（最长 30 秒），不重试 401/403 或掩盖已经中断的正文。`live_manifest_refresh_ms` 默认 250（50–30,000），限制同一直播清单的刷新频率，避免 FFmpeg 在下一片发布前空转耗尽重新加载次数。取消可中断等待。单请求总超时仍受 `MediaConfig.network_timeout` 限制。桌面媒体设置可选“直连 / 跟随全局 / 手动代理”；切换全局网络后，新任务等待现有任务结束才采用新策略，活动任务继续使用自己的原始配置。非法或损坏的设置会阻断任务，不静默回退。跟随全局时，如果全局代理声明了账号认证，媒体任务明确拒绝继承，要求选择独立网络配置；不会丢掉账号而继续访问。

认证、显式网络模式及直播使用每任务受控网关：先有界抓取、检查清单，将主清单、音轨、字幕、AES-128 KEY、MAP 初始化段与媒体 URI 全部换成随机 loopback 资源 ID。点播冻结检查后的清单；直播在 FFmpeg 重新请求清单时刷新，并对新增资源执行同样检查与累计预算。相同媒体资源复用 ID，保持显式及隐式 BYTERANGE 偏移；字节范围透传上游。FFmpeg 只接触 `127.0.0.1` 的随机能力地址，不接触上游账号、URL 查询或任意 URL 代理接口。服务校验 Host、拒绝 Origin/请求体/未知路径，关闭 keep-alive 并限制头部读取时间。媒体响应由 Hyper 连接直接持有上游流，背压由消费者控制，断开时释放上游；统一取消和等待所有连接后关闭监听。

上游每一跳都检查 HTTP(S) 与 URL 凭据禁令，禁止 HTTPS 降级为 HTTP；跨 origin 重定向会永久剥离该跳链中的 Authorization/Cookie/Referer，即使又回到原站也不恢复。独立子资源请求只使用其自身明确授权的 origin。Set-Cookie 不进入自动 cookie jar，服务器不能扩大授权。网关上游依赖 reqwest 默认 TLS 证书验证；没有不验证证书的配置。原 DRM、FFmpeg TLS/协议限制、诊断严格失败、空产物/缺片与安全发布校验全部保留。

DASH 网关展开 Representation 继承的 `SegmentTemplate` / `SegmentList` / `SegmentBase` 和单一路径 `BaseURL`，重写初始化、媒体及索引地址；`RepresentationID` / 带宽先做限制并展开，`Number` / `Time` 模板只接受有界十进制数值。请求不能通过模板替换任意路径或 URL。XML 深度和节点有界，DTD、外部引用、`xml:base`、xlink、Location/PatchLocation/UTCTiming/ContentSteering 以及多 BaseURL 均明确拒绝；不静默交还 FFmpeg 直连。LL-HLS、HLS 变量替换、未知 EXT-X 标签仍明确失败。

直播必须指定 `duration_limit`，可设置 `MediaConfig.live_finalize_grace_secs`（默认 3，范围 0–300）。录制期限内，尚未上传完成而返回 404 的直播片段每 250ms 重试；点播 404 直接失败。到达“录制时长＋收尾时间”后，网关仅对 HLS 直播媒体清单附加 ENDLIST，让稀疏字幕输入能够结束；输出进度不足指定时长（允许 250ms 音视频边界误差）返回 `IncompleteRecording`，不会将提前结束标为完成。DASH 动态清单继续刷新；编号模板保留原样寻址，有限 `SegmentTimeline` 则在网关精确展开 Number/Time 为 SegmentList，保留本次录制已见窗口且不改媒体字节，避开 FFmpeg 9.0.1 将分数秒音频边界取整和滑窗重编号的问题。历史条目受累计资源预算约束，窗口跳片、重复序号的时间变化、没有后续边界的 `r=-1` 无限重复均明确失败。限制录制时长使用 FFmpeg 输出时长参数。取消与失败都会等待网关连接及上游正文释放。

同机器同账户的其他进程不属于进程隔离威胁模型，不能用随机 loopback URL 代替操作系统账户边界。

## 恢复缓存

`MediaNetworkConfig.resume_cache_dir` 默认 `None`（关闭）。设置目录后，只在其专用 `swoop-media-cache-v1` 子目录保存完整媒体资源；`cache_max_mib` 默认 512（16–8192），`cache_resource_mib` 默认 16（1–64，且不超过总预算）。桌面媒体设置提供路径和预算；清单、AES 密钥及含结构化会话认证的下载不会落入缓存。

每个条目以原始 URL＋公开请求头（含 Range）的 SHA-256 为索引，保存正文哈希、强 ETag、最终重定向 URL 哈希和响应类型；不保存原始 URL、头部或凭据。每次重试/重启复用前先校验本地完整字节，再向源站发送 `If-None-Match`；只有同一最终 URL 的 304 才使用缓存，重定向目标变化时重新拉取正文。损坏、弱 ETag、超过单资源上限、no-store、Vary:* 或七天过期条目均不会复用。目录写入在 downloader 内串行并按预算清理可识别且完整的旧条目，超出有界目录扫描时停止新增缓存；不同进程共用目录的总量不提供全局锁保证。

这是已完成分片的校验恢复：未完成的片段重下，不承诺在任意字节处续接；直播已过源站时间窗的资源不能离线回放。缓存写入失败不影响已成功读取的网络正文。重启后的任务仍需应用显式重试，私有任务重新登录；不会自动删除用户文件或恢复备份。

## MMS 输入

MMS 不进入普通 HTTP 文件下载引擎。`MediaSource` 支持 `mms://`、`mmsh://`、`mmst://`；`MediaConfig.mms_transport` 为 `MmsTransport::Http`（默认）或 `Tcp`，只决定 `mms://` 映射到 MMSH/MMST，显式 scheme 保持原义。不自动从一种传输回退另一种。能力检查要求 FFmpeg 对应协议和 ASF demuxer，录制必须有时限、目标必须是 MKV；沿用取消、临时输出、同名保护和安全发布流程。

当前 MMS 仅支持直连，不接收结构化网页登录或代理配置，选择这些模式返回 `MmsNetworkUnsupported`。本机受控服务已经覆盖真正 MMSH 的双阶段 NSPlayer 请求、ASF 头和数据包，而不是把 ASF 文件伪装成普通 HTTP 下载。MMST 入口及 FFmpeg 能力检查已接入，旧 Windows Media Server、认证服务器和不同服务器方言仍需相应实机服务验收。

## 验证

普通单元测试无需 FFmpeg。真实集成测试显式标记 ignored，不会因为没装 FFmpeg 而伪装成功；带 `--ignored` 执行时缺工具会直接失败。

```bash
cargo test -p swoop-media
cargo test -p swoop-media --test real_media -- --ignored --nocapture
cargo clippy -p swoop-media --all-targets -- -D warnings
```

集成测试使用真实本机 HTTP 服务、由 FFmpeg 生成的 3 秒 H264/AAC HLS/DASH/AES-128 素材，下载后用 FFprobe 检查音视频流与时长；同时覆盖等待并发额度的取消、传输中的取消与临时清理、定稿时目标已存在，以及检查后远程清单切换为 `file://` 的防护。可用 `FFMPEG_BINARY`、`FFPROBE_BINARY` 指定下载/探测工具位置；夹具生成默认使用 PATH 的完整 `ffmpeg`，可另设 `FFMPEG_FIXTURE_BINARY`。

应用层已接入桌面/浏览器和任务历史；重启后公开媒体标记为已停止，私有媒体必须重新登录（已有失败信息和恢复路径继续保留），可从头重新下载。本库仍未实现：代理账号认证、TTML/封装 wvtt、未完成分片的任意字节续接、媒体带宽限制、FFmpeg 分发/更新、DRM 许可证内容；MMS 的代理/认证也未接入。Windows 代码路径和原生系统行为必须另外执行，macOS 结果不能替代 Windows 验收。

协议白名单及网络超时参数依据 [FFmpeg 官方协议说明](https://ffmpeg.org/ffmpeg-protocols.html)，no-clobber 使用 [tempfile API](https://docs.rs/tempfile/latest/tempfile/struct.TempPath.html#method.persist_noclobber)。


## 2026-10-11 历史验证记录

以下按功能开发先后保留当时记录；其中“未实现”仅描述对应历史批次。当前能力与限制以上文及末尾最新验证为准。

本机原有 `/opt/homebrew/bin/ffmpeg`、`ffprobe` 为 9.0.1；原 FFmpeg 实际只启用 HLS，缺少 DASH demuxer。能力检测因此明确返回 `FfmpegUnsupported("DASH 解复用器")`。没有安装 `ffmpeg-full` 的约 52 项额外依赖，也没有替换 Homebrew 默认程序。

为完成 DASH 的真实验收，下载并核对 [FFmpeg 官方 9.0.1 源码](https://ffmpeg.org/releases/ffmpeg-9.0.1.tar.xz)（SHA-256 `cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635`），仅在 `target/media-tools/` 下编译最小 HLS/DASH/MP4/MKV 工具，使用系统 libxml2 与 Secure Transport。没有系统安装操作。当前可执行文件：

- `target/media-tools/ffmpeg-9.0.1/ffmpeg`，5,344,264 字节，SHA-256 `71faaacdcd2071f69061b914a5f674ee2896f845ac86c0237957a9745a9836bf`。
- `target/media-tools/ffmpeg-9.0.1/ffprobe`，5,134,616 字节，SHA-256 `5b75804f9764705062d7129a761a0895ce2532c84259238714f4b557257e3a86`。

工具的实际 `-demuxers` 输出包含 HLS 和 DASH，`-h protocol=tls` 输出默认验证为 true。以上工具属于本机测试产物，不是已签名或已分发的应用依赖，不能硬编码为其他用户的默认路径。

最终真实集成测试命令（此前独立 manifest 已先通过编译，随后 workspace 接入由主代理处理）：

```bash
FFMPEG_BINARY="$PWD/target/media-tools/ffmpeg-9.0.1/ffmpeg" \
  cargo test --manifest-path "$(cat target/media-standalone-manifest.txt)" \
  --target-dir target/media-standalone --test real_media -- --ignored --nocapture
```

结果为 3 passed、0 failed。覆盖 HLS 主清单、AES-128 HLS、DASH 的真实 HTTP 下载；产物通过 FFprobe 音频/视频流与 3 秒时长验证；实际取消、并发排队、缺失分片、晚到目标文件及清单从 HTTP 媒体切换成 `file://` 均验证不会遗留完成文件或覆盖既有文件。普通检查为 5 个单元测试加 1 个本地 HTTP 清单预算/保护/循环检查；外部集成测试默认 ignored，不能只跑普通测试就宣称媒体实证完成。

中途发现并修复了 HTTP 输入不接受强设 `tls_verify`、FFmpeg 警告后跳过缺片仍退出 0、以及系统代理干扰本地 HTTP 等真实问题。Windows 实机、浏览器媒体捕获和桌面端实际操作没有由本库测试代验。

根 workspace 统一验证时已更新锁文件。`cargo test --workspace --locked` 中媒体 5 单元和 1 HTTP 检查通过；再设置上述 `FFMPEG_BINARY` 执行 `cargo test -p swoop-media --locked --test real_media -- --ignored --nocapture`，3 个真实媒体测试通过。日志保存在 `target/iteration-validation/real-media-tests.log`。

### 清晰度与独立音轨增量验证

新增真实 HTTP 多质量夹具：HLS fMP4 master 和 DASH 各包含 160×90 / 320×180 两路视频，以及 44.1 kHz 英文单声道 / 48 kHz 中文双声道音轨。分别下载“指定低清＋中文音轨”“自动最佳＋优先英文”“最高 100p＋无音轨”，六个产物均经 FFprobe 检查分辨率、声道数、采样率和约 3 秒时长；失效选择没有生成目标文件。

```bash
FFMPEG_BINARY="$PWD/target/media-tools/ffmpeg-9.0.1/ffmpeg" \
FFPROBE_BINARY="$PWD/target/media-tools/ffmpeg-9.0.1/ffprobe" \
cargo test -p swoop-media --test real_media -- --ignored --nocapture
```

结果：4 passed、0 failed（原三项安全/下载回归，加一项覆盖上述六个质量/音轨产物的测试）。普通媒体检查为 7 个单元测试、2 个 HTTP 夹具检查；应用测试确认选择随历史恢复、浏览器 FIFO 草稿保留及切换草稿取消/丢弃旧检查。`cargo clippy -p swoop-media --all-targets -- -D warnings` 通过。早期独立 TS 音频夹具含重复 DTS，被严格下载策略拒绝；没有关闭 `-xerror` 或放宽诊断标准来使其通过。有效 fMP4 音轨夹具通过后才确认选择功能。真实浏览器操作与 Windows 实机仍待单独验收。

### 夹具连接竞态修复

并行复验曾在清单检查阶段返回 `Network`（其余 3 项通过，总耗时 0.33 秒）。原因在测试 HTTP 服务器：监听 socket 使用非阻塞模式，本机 macOS 接受的新 socket 经 `fcntl` 实测仍有 `O_NONBLOCK`；处理器使用阻塞 `BufReader`，把请求还没到齐时的 `WouldBlock` 当作结束，提前关闭连接。新增 `fixture_accepts_delayed_and_split_request_headers`：连接后等待 60ms，发送请求行后再等 60ms 发送头部；修复前稳定报 `BrokenPipe`，修复后返回 HTTP 200 和完整响应体。

处理器现显式将接受的 socket 切回阻塞模式，并设置 3 秒读写超时。仅修改测试夹具，没有增加生产下载重试、隐藏错误或放宽媒体验收标准。修复后全 4 个真实媒体测试、7 个单元测试与 2 个 HTTP 检查、媒体严格 Clippy 均通过。独立 FFmpeg / FFprobe 的 SHA-256 与上文记录一致，实际版本为 9.0.1，HLS/DASH demuxer 均存在；素材生成使用 `/opt/homebrew/bin/ffmpeg` 9.0.1，未配置夹具工具覆盖或 mock。

### 字幕增量验证

为保持原工具不变，用同一已核对的官方源码在 `target/media-tools/subtitles/ffmpeg-9.0.1/` 构建独立测试工具，增加 WebVTT/mov_text/SubRip 编解码器、WebVTT/SRT 解复用，以及 MPEG-TS 到 MKV 所需的 `extract_extradata` bitstream filter。未安装系统依赖或替换 Homebrew 程序：

- `ffmpeg`：5,397,320 字节，SHA-256 `64d020b3e5a67c15039e71113163f17cc092c8f36a1f99c357927f6154900e62`。
- `ffprobe`：5,204,168 字节，SHA-256 `1ac2a8ea09e7964dc6f2e2cfa0c74348699bcd6aff2f8f718ef2d6288f962da9`。

```bash
FFMPEG_BINARY="$PWD/target/media-tools/subtitles/ffmpeg-9.0.1/ffmpeg" \
FFPROBE_BINARY="$PWD/target/media-tools/subtitles/ffmpeg-9.0.1/ffprobe" \
cargo test -p swoop-media --locked --test real_media -- --ignored --nocapture
```

结果：6 passed、0 failed，日志 `target/iteration-validation/media-subtitles-real.log`。原 4 项真实媒体回归继续通过；字幕新增用例覆盖 HLS fMP4 / MPEG-TS（1.4 秒非零时间基准和两片 WebVTT）、DASH 独立 WebVTT / mov_text 四种来源，每种分别生成 MP4 与 MKV，共 8 个字幕产物。FFprobe 检查编码、语言、视频分辨率、音频和总时长；每个产物再检查两条字幕在 0.2 / 1.5 秒开始、持续 1.0 / 1.3 秒，并重新解码确认中文字幕正文、没有英文和重复条目。每种来源默认关闭字幕时无字幕轨。负例验证失效选择、字幕第二片 404、整轨空字幕均不完成、不留目标文件。

普通媒体验证为 10 个单元测试、2 个 HTTP 夹具检查；6 个依赖真实工具的测试默认 ignored。`cargo clippy -p swoop-media --all-targets --locked -- -D warnings` 通过。旧任务缺字幕字段默认关闭，应用任务重载保留已选字幕；Windows、直播字幕和真实桌面交互未由上述库测试代验。

### 明确授权覆盖增量验证

`MediaRequest.overwrite` 默认为调用方显式传入 `false`；应用单次用户确认或全局覆盖策略才能传 `true`。新增 4 个出版校验单元测试，覆盖：默认拒绝、明确替换、相同字节保留 metadata/inode、用户改动、晚到目标、取消、目录/符号链接/父目录替换，以及单线程 Tokio 上 1 GiB 稀疏文件的后台校验取消。

真实 FFmpeg 新增覆盖测试确认：开始下载时旧文件仍在；成功发布后由 FFprobe 验证媒体；传输取消、缺片诊断失败、下载过程中用户写入新内容均保留原文件，不触发完成。`DestinationChanged` 由应用重新转为等待确认；`UnsafeDestination` 保持错误，不授权覆盖非普通文件。

最终上述独立字幕工具执行全部真实测试：**7 passed、0 failed**；普通媒体 **14 unit + 2 HTTP passed**；`cargo clippy -p swoop-media -p swoop-app --all-targets --locked -- -D warnings` 通过。日志在 `target/iteration-validation/media-final-{real,unit,clippy}.log`。应用媒体历史恢复测试同时通过。未执行 Windows 实机覆盖验收，也未将这些测试当作桌面弹窗交互验收。


### 发布与外部原子保存竞争回归

只做最后一次路径校验后再无条件替换，仍会丢失在检查与替换之间到达的用户原子保存。独立真实文件系统竞争夹具曾分别在第 19 / 73 次复现；修复采用上文的捕获实际目标、校验被移动文件、no-clobber 发布与恢复流程，没有仅增加一次路径检查或缩短窗口。

修复后同一独立夹具执行 **30,000 次**真实原子保存：`protected=29981, late_edits=3, recoveries=16`，每次目标均保留用户新内容；16 次被占用的恢复路径同时保留原文件。普通测试新增确定性边界检查（最后路径检查后外部保存、移动后取消与恢复目标已占用、提交后取消、已修改备份不得清理），另有 2,000 次无注入 hook 的真实原子保存竞争回归。

本轮验证命令与结果：

```bash
cargo test -p swoop-media --locked
# 19 unit + 2 HTTP passed; 7 个外部工具测试默认 ignored
FFMPEG_BINARY="$PWD/target/media-tools/subtitles/ffmpeg-9.0.1/ffmpeg" \
FFPROBE_BINARY="$PWD/target/media-tools/subtitles/ffmpeg-9.0.1/ffprobe" \
cargo test -p swoop-media --locked --test real_media -- --ignored --nocapture
# 7 passed；覆盖真实下载、音视频质量、字幕和覆盖/取消/用户改动保护
cargo clippy -p swoop-media --all-targets --locked -- -D warnings
# passed
cargo run --offline --manifest-path target/iteration-validation/media-readonly-review/Cargo.toml --target-dir target --quiet
# PASS 30000 atomic-save races; user edits retained in every attempt
```

日志分别为 `target/iteration-validation/media-publication-{unit,real,clippy,race-fixed}.log`。以上执行于 macOS；Windows 的恢复路径和原生文件占用行为仍待实机验收。应用须优先处理 `RecoveryRequired`，不能因取消同时到达将它隐藏为暂停。


### HLS 会话认证与受控网络增量验证

```bash
cargo test -p swoop-media -p swoop-app --locked
FFMPEG_BINARY="$PWD/target/media-tools/subtitles/ffmpeg-9.0.1/ffmpeg" \
FFPROBE_BINARY="$PWD/target/media-tools/subtitles/ffmpeg-9.0.1/ffprobe" \
cargo test -p swoop-media --locked --test real_media -- --ignored --nocapture
cargo clippy -p swoop-media -p swoop-app --all-targets --locked -- -D warnings
```

本轮最终结果：媒体 **23 单元 + 5 网络/认证服务夹具 + 2 HTTP 夹具通过**；全部 **10 个真实 FFmpeg 测试通过**；桌面 **89 通过、1 原生通知 smoke 默认忽略**，独立 CLI **11 通过**；上述两 crate 严格 Clippy 通过。日志为 `target/iteration-validation/media-gateway-final-{unit,real,clippy}.log`。最后仅将网络模式提示文字改为“此网络模式仅支持 HLS 点播”；父任务另做统一打包验证。

新增网络服务夹具实测同源 Authorization、Basic、Cookie 路径目录边界/过期/Secure 的 HTTP 拒发、跨端口 origin 重定向、跨源后重返原站继续剥离凭据、HTTP 代理与排除规则、SOCKS5h 远端域名握手、明确拒绝带认证 DASH/直播与取消。真实 FFmpeg 新增 3 个测试：认证 HLS master/AES-128/单文件 BYTERANGE、fMP4 MAP 初始化段、指定低清/英文音轨/中文字幕 MP4 与 MKV，产物由 FFprobe 检查编码/时长/字幕内容；缺片或取消不完成；使用不可直连的 `media.invalid` 通过手动 HTTP 代理完成 AES 密钥和媒体下载，服务端确认每一请求的目标 Host 与 Authorization。

网关生命周期 4 项单元检查包括能力路径/Host/Origin/HEAD、截断正文和不支持标签。新增真实挂起服务回归连续 12 次“上游已发响应头、后续正文永不完成、下游主动断开”：每次上游及时关闭，最大活动上游为 1，结束后全部并发额度归还；保持最后一条活动流时调用 shutdown 同样释放。实现为连接直接持有 `StreamBody`，没有脱离连接的正文 pump。应用回归还确认登录不进入历史、重启拒绝匿名继续、损坏设置不回退直连、修改 URL/外部预填/FIFO 清凭据与 origin 防复用，以及私有任务的恢复路径错误不会被登录状态隐藏。

本轮尚未进行 HTTPS 代理证书链的本地服务夹具、SOCKS5 本地 DNS 分支或原生 Windows/真实桌面点选验收；对应代理方案由 reqwest 实现且没有放宽证书验证，不能将 HTTP/SOCKS5h 的结果称作所有代理环境验收。浏览器原始 Cookie 仍拒绝接收，DASH/直播认证与代理、代理账号认证仍明确未实现。


### 媒体完成批次：DASH/直播认证、文本字幕、恢复缓存与 MMSH

当前实现和验证（后于上述历史批次）：

- 认证 DASH 点播经不可直连的代理目标完成低清视频、指定音轨、中文字幕；上游服务确认清单、初始化和媒体资源均带本次 origin 授权。
- HLS 直播真实刷新清单并限时录制；MPEG-TS HLS 直播的双片 WebVTT 和 DASH 独立 SRT/ASS 字幕通过正文、时间、语言及 MKV 编码检查。fMP4 HLS 直播字幕仍保留前述 FFmpeg 诊断兼容性边界。
- 独立实时 FFmpeg 发布器持续生成动态 DASH；使用三个片段宽的滑动时间线，认证录制 4.5 秒并取得初始窗口以外的第五片。FFprobe 检查音视频最后时间戳与总时长，逐包 DTS 严格递增；源端鉴权覆盖所有更新请求。网关精确展开有限时间线并限制刷新频率，未更改媒体时间戳、关闭诊断或修改 FFmpeg 源码。
- 强 ETag 完整分片缓存跨 downloader 重建后使用源站 304，未再次传输已确认片段正文；损坏缓存重新获取并生成可播放产物。单元同时验证弱 ETag、符号链接拒绝及最终重定向 URL 身份隔离。
- 本地受控 MMSH 服务完成描述/播放两阶段请求和真实 ASF 分帧，FFmpeg 录制到 MKV，FFprobe 确认 WMV2/WMA2 及约两秒时长；服务器关闭后的重试失败仍保留原文件。MMST 及各类旧服务器兼容性不由 MMSH 夹具代验。

同一已核对官方 9.0.1 源码的独立工具放在 `target/media-tools/completion/ffmpeg-9.0.1/`，增加 MMSH/MMST/ASF、ASS/SSA/SRT 所需模块；没有全局安装或替换 Homebrew：

- `ffmpeg`：5,720,040 字节，SHA-256 `f8e1fc984eed039251ce122d6f70b3c16a3dcf8b3e1e49e901179700a8ba797b`。
- `ffprobe`：5,526,904 字节，SHA-256 `cf21d9eff6b698275e3439aa55e05ac33adc019834f5ecfed22db82daaa9c6f6`。

```bash
cargo test -p swoop-media --locked
FFMPEG_BINARY="$PWD/target/media-tools/completion/ffmpeg-9.0.1/ffmpeg" \
FFPROBE_BINARY="$PWD/target/media-tools/completion/ffmpeg-9.0.1/ffprobe" \
cargo test -p swoop-media --locked --test real_media -- --ignored --nocapture
cargo clippy -p swoop-media --all-targets --locked -- -D warnings
```

最终结果：**26 单元 + 6 网络服务 + 2 HTTP 夹具通过；16 个真实 FFmpeg 测试全部通过；媒体严格 Clippy 通过**。日志为 `target/iteration-validation/media-completion-final-{unit,real,clippy}.log`。应用 `cargo test -p swoop-app --locked media_tasks::tests` 为 **5 passed**，包括全局带账号代理不得静默丢认证，日志 `media-completion-final-app.log`。真实测试使用上述二进制本身，诊断调试 wrapper 不用于最终验收。Windows 原生发布、进程取消、MMS/代理兼容和真实桌面操作仍需相应实机验证。
