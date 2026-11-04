import { mkdirSync } from "node:fs";
import { expect, test as setup } from "@playwright/test";
import { TID } from "./fixtures";
import { USERS, loginVia, sidCookie } from "./auth-helpers";
import { storageStateFor } from "../playwright.config";

// Runs once per engine (setup-chromium/-firefox/-webkit): signs in as
// console_user with that engine's own browser and saves storageState for that
// engine's authenticated projects only. Cookies are host-scoped (not
// port-scoped) and every E2E gateway shares the compose Postgres, so the
// session is valid on every E2E origin. Specs that sign out, rotate or expire
// a session use NO_AUTH / fresh contexts, never this one.

setup("sign in as console_user via Keycloak", async ({ page, baseURL, browserName }) => {
  const STATE = storageStateFor(browserName);
  mkdirSync(".auth", { recursive: true });
  await loginVia(page, baseURL!, "/", USERS.user);
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
  expect(await sidCookie(page.context())).toBeTruthy();
  await page.context().storageState({ path: STATE });
});
