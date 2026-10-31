import { expect, type Page, test } from "@playwright/test";

const SUBMIT = encodeURIComponent(
  "fn:webui/src/lib/components/organisms/spawn/SpawnForm.svelte#SpawnForm.submit",
);

const PAGES = {
  home: "?mock#/",
  map: "?mock&live=1#/p/shop",
  selection: `?mock#/p/dorskfr-cctui/node/${SUBMIT}`,
  story: `?mock#/p/dorskfr-cctui/story/${SUBMIT}`,
};

const SURFACES = [
  ".home",
  ".project-card",
  ".add-form",
  ".toolbar",
  ".search-wrap",
  ".right",
  ".legend",
  ".ticker",
  ".story-head",
  ".modal",
];

async function overflowing(page: Page): Promise<string[]> {
  return page.evaluate((selectors) => {
    const vw = window.innerWidth;
    const bad: string[] = [];
    if (document.documentElement.scrollWidth > vw) bad.push("document");
    for (const sel of selectors) {
      for (const el of document.querySelectorAll(sel)) {
        const r = el.getBoundingClientRect();
        if (!r.width) continue;
        if (r.left < -1 || r.right > vw + 1 || el.scrollWidth > el.clientWidth + 1)
          bad.push(`${sel} [${Math.round(r.left)}, ${Math.round(r.right)}]`);
      }
    }
    return bad;
  }, SURFACES);
}

for (const width of [390, 360]) {
  test.describe(`${width}px phone`, () => {
    test.use({ viewport: { width, height: 800 }, isMobile: true, hasTouch: true });

    for (const [name, path] of Object.entries(PAGES)) {
      test(`${name} fits the viewport`, async ({ page }) => {
        await page.goto(path);
        await page.waitForSelector(".home, .toolbar");
        await page.waitForTimeout(500);
        expect(await overflowing(page)).toEqual([]);
      });
    }

    test("toolbar controls are touch sized", async ({ page }) => {
      await page.goto(PAGES.map);
      const sizes = await page
        .locator(".toolbar button, .toolbar select, .toolbar input")
        .evaluateAll((els) =>
          els
            .map((el) => el.getBoundingClientRect())
            .filter((r) => r.width > 0)
            .map((r) => Math.min(r.width, r.height)),
        );
      expect(sizes.length).toBeGreaterThan(5);
      for (const s of sizes) expect(s).toBeGreaterThanOrEqual(44);
    });

    test("the flow picker fits", async ({ page }) => {
      await page.goto(PAGES.map);
      await page.getByRole("button", { name: "▶ Flows" }).click();
      await expect(page.locator(".modal")).toBeVisible();
      expect(await overflowing(page)).toEqual([]);
    });

    test("the node sheet closes", async ({ page }) => {
      await page.goto(PAGES.selection);
      const panel = page.locator(".node-panel");
      await expect(panel).toBeVisible();
      await panel.getByRole("button", { name: "Close" }).click();
      await expect(panel).toBeHidden();
    });
  });
}
