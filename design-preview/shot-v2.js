const { chromium } = require('playwright');

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1600, height: 900 } });
  // 延迟网络，确保能抓到开屏动画
  const client = await page.context().newCDPSession(page);
  await client.send('Network.emulateNetworkConditions', {
    offline: false, latency: 200, downloadThroughput: 400 * 1024, uploadThroughput: 400 * 1024,
  });
  await page.goto('http://localhost:5199/', { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(400);
  await page.screenshot({ path: __dirname + '/v5-splash.png' });
  await page.waitForTimeout(4000);
  await page.screenshot({ path: __dirname + '/v5-after.png' });
  const splashGone = await page.evaluate(() => !document.getElementById('splash'));
  console.log('splash removed after load:', splashGone);
  await browser.close();
})();
