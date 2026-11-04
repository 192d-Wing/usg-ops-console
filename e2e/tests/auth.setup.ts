import { mkdirSync } from "node:fs";
import { expect, test as setup } from "@playwright/test";
import { TID } from "./fixtures";
import { USERS, loginVia, sidCookie } from "./auth-helpers";

// Signs in once as console_user and saves storageState for the authenticated
// projects. Cookies are host-scoped (not port-scoped) and every E2E gateway
// shares the compose Postgres, so the one session is valid on every E2E origin.
const STATE = ".auth/console_user.json";

setup("sign in as console_user via Keycloak", async ({ page, baseURL }) => {
  mkdirSync(".auth", { recursive: true });
  await loginVia(page, baseURL!, "/", USERS.user);
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
  expect(await sidCookie(page.context())).toBeTruthy();
  await page.context().storageState({ path: STATE });
});
