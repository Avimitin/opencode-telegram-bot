import { Bot } from "grammy"
import { createOpencode } from "@opencode-ai/sdk/v2"
import { readFileSync, mkdirSync, readdirSync, rmSync } from "fs"
import { join } from "path"
import { homedir } from "os"
import { TOKEN, APPROVED_DIR } from "./config.js"
import { startSSESubscriber } from "./stream.js"
import { registerHandlers } from "./message.js"

// ── Opencode server ────────────────────────────────────────────────────────

console.log("Starting opencode server...")
const opencodeConfig = JSON.parse(
  readFileSync(join(process.env.HOME ?? homedir(), "opencode.json"), "utf8")
)
const opencode = await createOpencode({ port: 0, config: opencodeConfig })
console.log("Opencode server ready")

// ── Bot setup ──────────────────────────────────────────────────────────────

const bot = new Bot(TOKEN!)
let botUsername = ""

try {
  const me = await bot.api.getMe()
  botUsername = me.username ?? ""
  console.log(`Bot username: @${botUsername}`)
} catch (e) {
  console.error("Failed to get bot info:", e)
}

// ── Approved pairing polling ───────────────────────────────────────────────

setInterval(async () => {
  try {
    mkdirSync(APPROVED_DIR, { recursive: true })
    for (const file of readdirSync(APPROVED_DIR)) {
      const chatId = readFileSync(join(APPROVED_DIR, file), "utf8").trim()
      if (chatId) {
        await bot.api.sendMessage(chatId, "You have been approved! You can now send me messages.").catch(() => {})
      }
      rmSync(join(APPROVED_DIR, file))
    }
  } catch {}
}, 5000)

// ── Commands ───────────────────────────────────────────────────────────────

bot.api.setMyCommands([
  { command: "list_models", description: "List available models" },
  { command: "model", description: "Set model: /model provider/model" },
]).catch((e) => console.error("Failed to set commands:", e))

// ── SSE streaming ──────────────────────────────────────────────────────────

startSSESubscriber(opencode, bot)

// ── Message handlers ───────────────────────────────────────────────────────

registerHandlers(bot, opencode)

// ── Start ──────────────────────────────────────────────────────────────────

bot.catch((err) => {
  console.error("Bot error:", err)
})

await bot.start({
  onStart: () => console.log(`Telegram bot @${botUsername} is running!`),
})
