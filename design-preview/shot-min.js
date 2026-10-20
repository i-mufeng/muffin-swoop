const { chromium } = require('playwright');
const path = require('path');

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1600, height: 900 } });
  const dir = __dirname;
  for (const s of ['a', 'b', 'c']) {
    await page.goto('file:///' + path.join(dir, `min-${s}.html`).replace(/\\/g, '/'));
    await page.waitForTimeout(1200);
    await page.screenshot({ path: path.join(dir, `preview-min-${s}.png`) });
    console.log(`preview-min-${s}.png done`);
  }
  await browser.close();
})();
