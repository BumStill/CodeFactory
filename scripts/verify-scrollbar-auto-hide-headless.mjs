// Headless verification: session list scrollbar-auto-hide class is rendered in real DOM.
// Run this against an already-running Vite dev server on port 1420 (pnpm dev).
// SPDX-License-Identifier: Apache-2.0

import { chromium } from 'playwright-core';
import http from 'http';

const VITE_URL = 'http://localhost:1420';

function waitForHttp(url, timeoutMs = 15_000) {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    const poll = () => {
      http.get(url, (res) => {
        if (res.statusCode === 200) return resolve();
        if (Date.now() - start < timeoutMs) return setTimeout(poll, 500);
        reject(new Error(`HTTP ${res.statusCode} after ${timeoutMs}ms`));
      }).on('error', () => {
        if (Date.now() - start < timeoutMs) return setTimeout(poll, 500);
        reject(new Error('Vite not ready after ' + timeoutMs + 'ms'));
      });
    };
    poll();
  });
}

async function main() {
  console.log('Verifying Vite is up...');
  await waitForHttp(VITE_URL);
  console.log('Vite ready.');

  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const page = await browser.newPage();

  try {
    await page.goto(VITE_URL, { waitUntil: 'domcontentloaded', timeout: 15_000 });
    console.log('Page loaded.');
    await page.waitForTimeout(2000);

    const classCount = await page.locator('.scrollbar-auto-hide').count();
    console.log('scrollbar-auto-hide elements:', classCount);

    if (classCount === 0) {
      const bodyText = await page.locator('body').innerText();
      console.log('Page body (first 500 chars):', bodyText.substring(0, 500));
      console.error('FAIL: scrollbar-auto-hide class NOT found in rendered DOM');
      process.exit(1);
    }

    const matched = await page.locator('.scrollbar-auto-hide.overflow-y-auto').count();
    console.log('scrollbar-auto-hide + overflow-y-auto combined:', matched);

    if (matched === 0) {
      console.error('FAIL: scrollbar-auto-hide not on overflow-y-auto container');
      process.exit(1);
    }

    const hasStyle = await page.evaluate(() => {
      for (const sheet of document.styleSheets) {
        try {
          for (const rule of sheet.cssRules) {
            if (rule.cssText.includes('.scrollbar-auto-hide::-webkit-scrollbar-thumb')) {
              return true;
            }
          }
        } catch {}
      }
      return false;
    });

    console.log('CSS rule present in stylesheet:', hasStyle);

    if (!hasStyle) {
      console.error('FAIL: .scrollbar-auto-hide CSS rule not found in stylesheets');
      process.exit(1);
    }

    console.log('PASS: scrollbar-auto-hide verified in rendered DOM + loaded CSS');
  } finally {
    await browser.close();
  }
}

main().catch((err) => {
  console.error('FATAL:', err.message);
  process.exit(2);
});
