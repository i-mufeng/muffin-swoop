# swoop.muffin.xin 官网

Swoop 下载管理器的官方网站。Vue 3 + Vite + TypeScript，独立于主仓库构建。

## 开发

```bash
npm install
npm run dev
```

## 发布

```bash
bash deploy.sh           # 构建并上传
bash deploy.sh --build   # 只构建
```

部署服务器（默认约定，未单独指定时都用它）：
`ssh root@ms.imufeng.cn -p 15990`，站点目录 `/data/web-service/sites/swoop.muffin.xin/index/`。

## 数据与素材约定

- **版本号 / 下载地址**：运行时读取 `https://cdn.muffin.xin/swoop-release/feed.json`
  （签名更新清单），失败回退 GitHub API。发布新版本并同步 CDN latest 后网站自动更新。
- **截图素材**：`public/shots/home-{dark,light}-{zh,en}.png` 共 4 张，
  用仓库根目录的 `scripts/screenshot.sh` + `SWOOP_LANG` 环境变量重新生成：
  ```bash
  SWOOP_LANG=zh-CN bash scripts/screenshot.sh home dark website/public/shots/home-dark-zh.png
  ```
- **背景图 / 图标**：`public/bg-dark.png`（深色模式淡点阵）、`public/icon.png`。
- 下载直链兜底地址等集中在 `src/config.ts`。
