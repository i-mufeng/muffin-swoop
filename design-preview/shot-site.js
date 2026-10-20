const { chromium } = require('playwright');

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1600, height: 900 } });
  await page.goto('http://localhost:5199/', { waitUntil: 'networkidle' });
  await page.waitForTimeout(2000);

  // 当前主题（跟随系统，headless 默认 light）
  await page.evaluate(() => document.querySelector('#features').scrollIntoView());
  await page.waitForTimeout(600);
  await page.screenshot({ path: __dirname + '/sec-features.png' });
  await page.evaluate(() => document.querySelector('#download').scrollIntoView());
  await page.waitForTimeout(600);
  await page.screenshot({ path: __dirname + '/sec-download.png' });

  // 切到深色再截 hero + features
  await page.click('.theme-btn');
  await page.waitForTimeout(1000);
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.mouse.move(800, 450);
  await page.waitForTimeout(600);
  await page.screenshot({ path: __dirname + '/sec-hero-dark.png' });
  await page.evaluate(() => document.querySelector('#features').scrollIntoView());
  await page.waitForTimeout(600);
  await page.screenshot({ path: __dirname + '/sec-features-dark.png' });
  console.log('done');
  await browser.close();
})();
