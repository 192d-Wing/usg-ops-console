import { request } from "@playwright/test";
import { KEYCLOAK } from "./auth-helpers";

/**
 * DEV ONLY Keycloak admin API helpers (compose bootstrap admin, see
 * deploy/compose/compose.yaml). Used by revocation specs to disable a user or
 * drop a role mid-session (NIST-001 / AC-2).
 */
const ADMIN_USER = process.env.KC_ADMIN_USER ?? "admin";
const ADMIN_PASSWORD = process.env.KC_ADMIN_PASSWORD ?? "dev-only-admin";
const REALM = "icam";

async function api() {
  const tokenCtx = await request.newContext({ baseURL: KEYCLOAK });
  const res = await tokenCtx.post("/realms/master/protocol/openid-connect/token", {
    form: { grant_type: "password", client_id: "admin-cli", username: ADMIN_USER, password: ADMIN_PASSWORD },
  });
  if (!res.ok()) throw new Error(`kc admin token: ${res.status()}`);
  const { access_token } = (await res.json()) as { access_token: string };
  await tokenCtx.dispose();
  return request.newContext({
    baseURL: `${KEYCLOAK}/admin/realms/${REALM}/`,
    extraHTTPHeaders: { Authorization: `Bearer ${access_token}` },
  });
}

async function userId(ctx: Awaited<ReturnType<typeof api>>, username: string): Promise<string> {
  const res = await ctx.get(`users?exact=true&username=${encodeURIComponent(username)}`);
  const users = (await res.json()) as { id: string }[];
  const id = users[0]?.id;
  if (!id) throw new Error(`kc user ${username} not found`);
  return id;
}

export async function setUserEnabled(username: string, enabled: boolean): Promise<void> {
  const ctx = await api();
  try {
    const id = await userId(ctx, username);
    const res = await ctx.put(`users/${id}`, { data: { enabled } });
    if (!res.ok()) throw new Error(`kc enable=${enabled} ${username}: ${res.status()}`);
  } finally {
    await ctx.dispose();
  }
}

export async function setRealmRole(username: string, role: string, present: boolean): Promise<void> {
  const ctx = await api();
  try {
    const id = await userId(ctx, username);
    const r = await ctx.get(`roles/${encodeURIComponent(role)}`);
    const rep = await r.json();
    const path = `users/${id}/role-mappings/realm`;
    const res = present ? await ctx.post(path, { data: [rep] }) : await ctx.delete(path, { data: [rep] });
    if (!res.ok()) throw new Error(`kc role ${role} present=${present} ${username}: ${res.status()}`);
  } finally {
    await ctx.dispose();
  }
}
