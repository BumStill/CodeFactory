// Headless verification: interactive elements meet the minimum hit area.
//
// Run against an already-running Vite dev server (pnpm dev), or point
// CODEFACTORY_VITE_URL at one — port 1420 is strictPort and parallel worktrees
// compete for it, so a run can otherwise silently measure another checkout.
//
// WCAG 2.2 SC 2.5.8 Target Size (Minimum) is 24x24 CSS px. jsdom has no layout,
// so unit tests cannot see this at all: the usage trend shipped with every bar
// as its own <button>, which made a zero-usage day a 4x11px click target while
// every test passed.
//
// See docs/specs/ui-typography-and-spacing.md 第六节.
// SPDX-License-Identifier: Apache-2.0

import { chromium } from 'playwright-core';
import http from 'http';

const VITE_URL = process.env.CODEFACTORY_VITE_URL ?? 'http://localhost:1420';
const MIN_TARGET_PX = 24;

/**
 * Dense data grids that cannot give every cell 24px without destroying the
 * visualisation — SC 2.5.8 "Essential" exception. Both keep an equivalent
 * accessible path: full arrow-key navigation, and per-day aria-labels.
 *
 * Registering them here is deliberate: an exception someone has to write down
 * and justify is one that gets revisited; a threshold quietly lowered is not.
 */
const ESSENTIAL_EXCEPTIONS = [
  {
    selector: '[aria-label*="趋势"] [role="gridcell"]',
    exempt: 'width',
    why: '4 周趋势图：28 列排在 ~280px 内，每列约 10px 宽；高度仍须达标',
  },
  {
    selector: '[aria-label*="消耗地图"] [role="gridcell"]',
    exempt: 'both',
    why: 'Token 消耗地图：日历热力图格子，两个方向都受网格约束',
  },
];

function waitForHttp(url, timeoutMs = 15_000) {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    const poll = () => {
      http
        .get(url, (res) => {
          if (res.statusCode === 200) return resolve();
          if (Date.now() - start < timeoutMs) return setTimeout(poll, 500);
          reject(new Error(`HTTP ${res.statusCode} after ${timeoutMs}ms`));
        })
        .on('error', () => {
          if (Date.now() - start < timeoutMs) return setTimeout(poll, 500);
          reject(new Error(`Vite not ready after ${timeoutMs}ms`));
        });
    };
    poll();
  });
}

const PAGES = ['usage-acceptance.html', 'sidebar-expansion-acceptance.html', 'draft-project-picker-acceptance.html'];

async function main() {
  await waitForHttp(VITE_URL);
  console.log('Vite ready.');

  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const page = await browser.newPage();
  await page.setViewportSize({ width: 1440, height: 900 });
  const failures = [];

  try {
    for (const path of PAGES) {
      await page.goto(`${VITE_URL}/${path}`, { waitUntil: 'domcontentloaded', timeout: 15_000 });
      await page.waitForTimeout(1200);

      const undersized = await page.evaluate(
        ([min, exceptions]) => {
          // Exemptions are per-axis. A visualisation that constrains column
          // width has no claim on height, and exempting the whole element
          // would have hidden the 4px-tall trend bars this check exists for.
          const exempt = new Map();
          for (const { selector, exempt: axis } of exceptions) {
            document.querySelectorAll(selector).forEach((node) => exempt.set(node, axis));
          }
          const found = [];
          document.querySelectorAll('button,a[href],[role="button"],input:not([type="hidden"]),select').forEach((el) => {
            const axis = exempt.get(el);
            if (axis === 'both') return;
            const rect = el.getBoundingClientRect();
            // Hidden elements have no hit area to speak of.
            if (rect.width === 0 || rect.height === 0) return;
            const widthOk = axis === 'width' || rect.width >= min;
            const heightOk = axis === 'height' || rect.height >= min;
            if (widthOk && heightOk) return;
            found.push({
              tag: el.tagName.toLowerCase(),
              w: Math.round(rect.width * 10) / 10,
              h: Math.round(rect.height * 10) / 10,
              label: (el.getAttribute('aria-label') || el.textContent || '').trim().slice(0, 20),
            });
          });
          return found;
        },
        [MIN_TARGET_PX, ESSENTIAL_EXCEPTIONS],
      );

      if (undersized.length === 0) {
        console.log(`  ok   ${path}`);
      } else {
        console.error(`  FAIL ${path}`);
        for (const item of undersized) {
          console.error(`         ${item.w}x${item.h}  <${item.tag}> 「${item.label}」`);
          failures.push(`${path} ${item.tag} ${item.w}x${item.h}`);
        }
      }
    }
  } finally {
    // Always close — a verify run that threw before this once left a headless
    // Chrome pinned at ~100% CPU for five days.
    await browser.close();
  }

  console.log('\n登记在案的 Essential 例外:');
  for (const { why } of ESSENTIAL_EXCEPTIONS) console.log(`  - ${why}`);

  if (failures.length > 0) {
    console.error(`\nFAIL: ${failures.length} 个交互元素命中区小于 ${MIN_TARGET_PX}px`);
    process.exit(1);
  }
  console.log(`\nPASS: 所有交互元素命中区 >= ${MIN_TARGET_PX}px`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
