import { expect, test } from "@playwright/test";
import { TID } from "./fixtures";
import { NO_AUTH, loginAt } from "./auth-helpers";
import { TOOL_TID, launch, toolFrameEl } from "./tool-helpers";

// console-user WITHOUT hello:user (realm user nohello_user).
test.use({ storageState: NO_AUTH });

test.describe("user without hello:user", () => {
  test("SEC-011: launch of a forbidden tool and of an unknown id return identical 404 status, headers and body", async ({ page, baseURL }) => {
    const nohello = { username: "nohello_user", password: "dev-only-nohello_user", name: /nohello_user|No Hello/ };
    await loginAt(page, baseURL!, "/", nohello);
    // Per-response values that legitimately differ between any two responses.
    const VOLATILE = new Set(["date", "x-request-id", "traceparent"]);
    const shape = async (id: string) => {
      const r = await launch(page.request, id);
      const h = Object.fromEntries(
        Object.entries(r.headers()).filter(([k]) => !VOLATILE.has(k.toLowerCase())).sort(([a], [b]) => a.localeCompare(b)),
      );
      return { status: r.status(), headers: h, body: await r.text() };
    };
    const forbidden = await shape("hello");
    const unknown = await shape("no-such-tool-sec011");
    expect(forbidden.status).toBe(404);
    expect(forbidden, "forbidden vs unknown launch response").toEqual(unknown);
  });

  test("hello hidden from the service menu; launch and /tools/hello are 404 Tool not found (INT-009)", async ({ page, baseURL }) => {
    const nohello = { username: "nohello_user", password: "dev-only-nohello_user", name: /nohello_user|No Hello/ };
    await loginAt(page, baseURL!, "/", nohello);
    const tools = await (await page.request.get("/api/tools", { headers: { Accept: "application/json" } })).json();
    expect((tools.tools as { id: string }[]).map((t) => t.id)).not.toContain("hello");
    await page.getByTestId(TID.serviceMenuButton).click();
    await expect(page.getByTestId(TID.serviceMenuPanel).getByTestId("service-menu-item-tool-hello")).toHaveCount(0);
    const r = await launch(page.request, "hello");
    expect(r.status()).toBe(404);
    expect(await r.json()).toMatchObject({ error: "not_found" });
    await page.goto("/tools/hello/");
    await expect(page.getByTestId(TOOL_TID.notFound)).toBeVisible();
    await expect(page.getByRole("heading", { name: /tool not found/i })).toBeVisible();
    await expect(page.getByTestId(TOOL_TID.forbidden)).toHaveCount(0);
    await expect(toolFrameEl(page)).toHaveCount(0);
  });
  });
