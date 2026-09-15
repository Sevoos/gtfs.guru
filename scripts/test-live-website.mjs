#!/usr/bin/env node

import assert from 'node:assert/strict';
import { chromium } from 'playwright';

const siteUrl = (process.env.GTFS_GURU_SITE_URL || 'https://gtfs.guru').replace(/\/+$/, '');
const expectedVersion = process.env.EXPECTED_VERSION?.replace(/^v/, '');
const requireIsolation = process.env.REQUIRE_CROSS_ORIGIN_ISOLATION !== 'false';
const cacheBuster = `smoke=${Date.now()}`;

async function fetchSite(path) {
  const separator = path.includes('?') ? '&' : '?';
  const url = `${siteUrl}${path}${separator}${cacheBuster}`;
  const response = await fetch(url, {
    cache: 'no-store',
    signal: AbortSignal.timeout(60_000),
  });
  assert.equal(response.ok, true, `${url} returned HTTP ${response.status}`);
  return response;
}

const packageJson = await fetchSite('/pkg/package.json').then((response) => response.json());
if (expectedVersion) {
  assert.equal(
    packageJson.version,
    expectedVersion,
    `deployed package version is ${packageJson.version}, expected ${expectedVersion}`,
  );
}

await Promise.all([
  '/pkg/worker.js',
  '/pkg-mt/worker-mt.js',
  '/demo/gtfs-guru-demo.zip',
  '/notices/',
  '/llms.txt',
].map(async (path) => {
  const response = await fetchSite(path);
  await response.arrayBuffer();
  return path;
}));

const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage();
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));

  await page.goto(`${siteUrl}/?${cacheBuster}`, {
    waitUntil: 'domcontentloaded',
    timeout: 60_000,
  });
  if (requireIsolation) {
    assert.equal(
      await page.evaluate(() => globalThis.crossOriginIsolated),
      true,
      'the production page is missing cross-origin isolation headers',
    );
  }

  await page.getByRole('button', { name: /validate an example feed/i }).click();
  await page.waitForFunction(
    () => {
      const result = document.querySelector('#result-state');
      const error = document.querySelector('#error-container');
      return !result?.classList.contains('hidden') || !error?.classList.contains('hidden');
    },
    undefined,
    { timeout: 120_000 },
  );

  const state = await page.evaluate(() => ({
    resultVisible: !document.querySelector('#result-state')?.classList.contains('hidden'),
    errorVisible: !document.querySelector('#error-container')?.classList.contains('hidden'),
    errorMessage: document.querySelector('#error-message')?.textContent?.trim() || '',
    errorCount: document.querySelector('#error-count')?.textContent?.trim() || '',
    warningCount: document.querySelector('#warning-count')?.textContent?.trim() || '',
    verdict: document.querySelector('#completion-text')?.textContent?.trim() || '',
    verdictState: document.querySelector('#completion-indicator')?.className || '',
    issueCards: document.querySelectorAll('#result-issues .issue-card').length,
    firstIssueDocLink: document.querySelector('.issue-doc-link')?.getAttribute('href') || '',
    mcpPreviewPresent: !!document.querySelector('#mcp-preview'),
    mcpExampleCount: document.querySelectorAll('.mcp-example').length,
    mcpVerdict: document.querySelector('.mcp-verdict')?.textContent?.trim() || '',
  }));

  assert.equal(
    state.errorVisible,
    false,
    `example-feed validation failed: ${state.errorMessage || 'unknown browser error'}`,
  );
  assert.equal(state.resultVisible, true, 'example-feed validation produced no result');
  assert.match(state.errorCount, /^\d+$/, 'the result has no numeric error count');
  assert.match(state.warningCount, /^\d+$/, 'the result has no numeric warning count');
  // The example feed carries deliberate errors, so the verdict must say so.
  // A green "scan complete" over a failing feed is the regression this guards.
  assert.equal(
    Number(state.errorCount) > 0,
    true,
    'the example feed is supposed to contain errors',
  );
  assert.match(
    state.verdict,
    /errors? need fixing/i,
    `the verdict does not report the errors: ${state.verdict}`,
  );
  assert.match(
    state.verdictState,
    /state-error/,
    `the verdict is not in its error state: ${state.verdictState}`,
  );
  assert.equal(state.issueCards > 0, true, 'the report listed no issues inline');
  assert.match(
    state.firstIssueDocLink,
    /^\/notices\/[a-z0-9_]+\/$/,
    `the first issue does not link to its notice page: ${state.firstIssueDocLink}`,
  );
  assert.equal(state.mcpPreviewPresent, true, 'the MCP preview is missing from the result');
  assert.equal(state.mcpExampleCount > 0, true, 'the demo feed produced no MCP error examples');
  assert.match(state.mcpVerdict, /I checked gtfs-guru-demo/);
  assert.deepEqual(pageErrors, [], `page errors: ${pageErrors.join('; ')}`);

  console.log(JSON.stringify({
    siteUrl,
    version: packageJson.version,
    exampleFeed: {
      errors: Number(state.errorCount),
      warnings: Number(state.warningCount),
    },
  }, null, 2));
} finally {
  await browser.close();
}
