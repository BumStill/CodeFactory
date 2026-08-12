// Headless verification: the type scale and the 4px grid render at their
// authored values in a real browser.
//
// Run against an already-running Vite dev server on port 1420 (pnpm dev).
//
// This exists because the defect it guards was invisible to unit tests. jsdom
// reports whatever font-size string the class map claims; only a real engine
// resolves `0.75rem` against the root font size. With the root pinned to the
// user's 14px text size, `text-xs` resolved to 10.5px and `gap-2` to 7px —
// every vitest assertion still passed while 83% of the product's text sat
// between 10 and 11px and card titles rendered smaller than their own body.
//
// See docs/specs/ui-typography-and-spacing.md.
// SPDX-License-Identifier: Apache-2.0

import { chromium } from 'playwright-core';
import http from 'http';

// Configurable because 1420 is `strictPort` and parallel worktrees compete for
// it. A run that silently reaches another checkout's dev server reports that
// checkout's CSS as if it were this one's — this verification first measured a
// sibling worktree and declared every assertion broken.
const VITE_URL = process.env.CODEFACTORY_VITE_URL ?? 'http://localhost:1420';

/** Authored px for each semantic step; must match tailwind.config.js. */
const TYPE_SCALE = {
  caption: 11,
  label: 12,
  note: 13,
  body: 14,
  reading: 15,
  title: 16,
  heading: 18,
  display: 22,
};

/** The 4px grid. These must not move when the user changes text size. */
const GRID = {
  'gap-1': 4,
  'gap-1.5': 6,
  'gap-2': 8,
  'gap-3': 12,
  'gap-4': 16,
  'p-3': 12,
  'p-4': 16,
  rounded: 4,
  'rounded-lg': 8,
  'rounded-xl': 12,
};

/** Nothing readable may render below this. */
const MIN_TEXT_PX = 11;

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

const failures = [];
const check = (label, actual, expected) => {
  if (actual === expected) console.log(`  ok   ${label}: ${actual}px`);
  else {
    console.error(`  FAIL ${label}: ${actual}px, 期望 ${expected}px`);
    failures.push(`${label}: ${actual} != ${expected}`);
  }
};

/** Measure a class by mounting a probe element and reading the used value. */
const measure = (page, classes, property) =>
  page.evaluate(
    ([className, prop]) => {
      const el = document.createElement('div');
      el.className = className;
      el.textContent = 'Ag中文';
      document.body.appendChild(el);
      const value = parseFloat(getComputedStyle(el)[prop]);
      el.remove();
      return value;
    },
    [classes, property],
  );

async function main() {
  await waitForHttp(VITE_URL);
  console.log('Vite ready.');

  const browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const page = await browser.newPage();

  try {
    await page.goto(`${VITE_URL}/usage-acceptance.html`, { waitUntil: 'domcontentloaded', timeout: 15_000 });
    await page.waitForTimeout(1500);

    console.log('\n[1] rem 基准未被应用改写');
    const rootFontSize = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize));
    check('html font-size', rootFontSize, 16);

    console.log('\n[2] 语义字号渲染为设计值');
    for (const [token, expected] of Object.entries(TYPE_SCALE)) {
      check(`text-${token}`, await measure(page, `text-${token}`, 'fontSize'), expected);
    }

    console.log('\n[3] 间距与圆角落在 4px 网格上');
    for (const [className, expected] of Object.entries(GRID)) {
      const property = className.startsWith('gap') ? 'gap' : className.startsWith('p-') ? 'paddingTop' : 'borderTopLeftRadius';
      check(className, await measure(page, `flex ${className}`, property), expected);
    }

    console.log('\n[4] 字号设置只缩放文字，不缩放布局');
    await page.evaluate(() => document.documentElement.style.setProperty('--font-scale', String(20 / 14)));
    const scaledText = await measure(page, 'text-body', 'fontSize');
    const scaledGap = await measure(page, 'flex gap-2', 'gap');
    const scaledRadius = await measure(page, 'rounded-lg', 'borderTopLeftRadius');
    check('text-body @ 20px 设置', Math.round(scaledText), 20);
    check('gap-2 不随字号变', scaledGap, 8);
    check('rounded-lg 不随字号变', scaledRadius, 8);
    await page.evaluate(() => document.documentElement.style.removeProperty('--font-scale'));

    console.log('\n[5] 真实挂载页面上没有低于下限的文字');
    const tooSmall = await page.evaluate((floor) => {
      const found = [];
      document.querySelectorAll('*').forEach((el) => {
        const text = [...el.childNodes]
          .filter((node) => node.nodeType === 3)
          .map((node) => node.textContent.trim())
          .join('')
          .trim();
        if (!text) return;
        const size = parseFloat(getComputedStyle(el).fontSize);
        if (size < floor) found.push({ text: text.slice(0, 16), size });
      });
      return found;
    }, MIN_TEXT_PX);
    if (tooSmall.length === 0) console.log(`  ok   全部文字 >= ${MIN_TEXT_PX}px`);
    else {
      for (const item of tooSmall) console.error(`  FAIL ${item.size}px 「${item.text}」`);
      failures.push(`${tooSmall.length} 处文字低于 ${MIN_TEXT_PX}px`);
    }

    console.log('\n[6] 卡片标题不小于它自己的正文');
    const inverted = await page.evaluate(() => {
      const bad = [];
      document.querySelectorAll('h1,h2,h3,h4').forEach((heading) => {
        const headingSize = parseFloat(getComputedStyle(heading).fontSize);
        const container = heading.closest('section,article,div');
        if (!container) return;
        for (const sibling of container.querySelectorAll('p,div,span')) {
          if (heading.contains(sibling)) continue;
          // Only elements that own their text. A wrapper `<div>` with no
          // font-size class inherits the 14px body default while its
          // `textContent` reports its children's copy — comparing against that
          // flags every heading below 14px, which is a property of the
          // wrapper, not a hierarchy defect.
          const ownText = [...sibling.childNodes]
            .filter((node) => node.nodeType === 3)
            .map((node) => node.textContent.trim())
            .join('')
            .trim();
          if (!ownText) continue;
          const siblingSize = parseFloat(getComputedStyle(sibling).fontSize);
          if (siblingSize > headingSize) {
            bad.push({ heading: heading.textContent.trim().slice(0, 12), headingSize, siblingSize });
            break;
          }
        }
      });
      return bad;
    });
    if (inverted.length === 0) console.log('  ok   没有层级倒挂');
    else {
      for (const item of inverted) console.error(`  FAIL 「${item.heading}」${item.headingSize}px < 同容器正文 ${item.siblingSize}px`);
      failures.push(`${inverted.length} 处层级倒挂`);
    }
  } finally {
    // Always close. A verify run that threw before this line once left a
    // headless Chrome pinned at ~100% CPU for five days.
    await browser.close();
  }

  if (failures.length > 0) {
    console.error(`\nFAIL: ${failures.length} 项不符合规范`);
    process.exit(1);
  }
  console.log('\nPASS: 排版与网格符合 docs/specs/ui-typography-and-spacing.md');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
