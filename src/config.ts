import { readFileSync } from "fs"
import { join } from "path"
import { homedir } from "os"

// ── Paths ──────────────────────────────────────────────────────────────────

export const STATE_DIR = process.env.TELEGRAM_STATE_DIR ?? join(homedir(), ".opencode", "channels", "telegram")
export const ACCESS_FILE = join(STATE_DIR, "access.json")
export const APPROVED_DIR = join(STATE_DIR, "approved")
const ENV_FILE = join(STATE_DIR, ".env")

// ── Load .env for bot token ────────────────────────────────────────────────

try {
  for (const line of readFileSync(ENV_FILE, "utf8").split("\n")) {
    const m = line.match(/^(\w+)=(.*)$/)
    if (m && process.env[m[1]!] === undefined) process.env[m[1]!] = m[2]!
  }
} catch {}

export const TOKEN = process.env.TELEGRAM_BOT_TOKEN
if (!TOKEN) {
  console.error(
    `telegram channel: TELEGRAM_BOT_TOKEN required\n` +
    `  set in ${ENV_FILE}\n` +
    `  format: TELEGRAM_BOT_TOKEN=123456789:AAH...\n`,
  )
  process.exit(1)
}
