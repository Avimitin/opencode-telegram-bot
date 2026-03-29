import { Bot, type Context } from "grammy"
import { createOpencode, type ToolPart } from "@opencode-ai/sdk"
import { readFileSync, writeFileSync, mkdirSync, existsSync, readdirSync, rmSync } from "fs"
import { join } from "path"
import { homedir } from "os"
import { randomBytes } from "crypto"

// ── Config ──────────────────────────────────────────────────────────────────

const STATE_DIR = process.env.TELEGRAM_STATE_DIR ?? join(homedir(), ".opencode", "channels", "telegram")
const ACCESS_FILE = join(STATE_DIR, "access.json")
const APPROVED_DIR = join(STATE_DIR, "approved")
const ENV_FILE = join(STATE_DIR, ".env")

// Load .env for bot token
try {
  for (const line of readFileSync(ENV_FILE, "utf8").split("\n")) {
    const m = line.match(/^(\w+)=(.*)$/)
    if (m && process.env[m[1]!] === undefined) process.env[m[1]!] = m[2]!
  }
} catch {}

const TOKEN = process.env.TELEGRAM_BOT_TOKEN
if (!TOKEN) {
  console.error(
    `telegram channel: TELEGRAM_BOT_TOKEN required\n` +
    `  set in ${ENV_FILE}\n` +
    `  format: TELEGRAM_BOT_TOKEN=123456789:AAH...\n`,
  )
  process.exit(1)
}

// ── Access Control ──────────────────────────────────────────────────────────

type PendingEntry = {
  senderId: string
  chatId: string
  createdAt: number
  expiresAt: number
  replies: number
}

type GroupPolicy = {
  requireMention: boolean
  allowFrom: string[]
}

type Access = {
  dmPolicy: "pairing" | "allowlist" | "disabled"
  allowFrom: string[]
  groups: Record<string, GroupPolicy>
  pending: Record<string, PendingEntry>
  mentionPatterns?: string[]
}

function loadAccess(): Access {
  try {
    return JSON.parse(readFileSync(ACCESS_FILE, "utf8"))
  } catch {
    return { dmPolicy: "pairing", allowFrom: [], groups: {}, pending: {} }
  }
}

function saveAccess(access: Access) {
  mkdirSync(STATE_DIR, { recursive: true })
  writeFileSync(ACCESS_FILE, JSON.stringify(access, null, 2))
}

// ── Pairing ─────────────────────────────────────────────────────────────────

function generatePairingCode(): string {
  return randomBytes(3).toString("hex")
}

function handlePairing(senderId: string, chatId: string): string {
  const access = loadAccess()

  // Already allowed
  if (access.allowFrom.includes(senderId)) {
    return "You are already paired. Send messages and I will respond."
  }

  // Check existing pending
  const existing = Object.entries(access.pending).find(([, v]) => v.senderId === senderId)
  if (existing) {
    const [code, entry] = existing
    if (entry.expiresAt > Date.now()) {
      entry.replies = (entry.replies || 0) + 1
      if (entry.replies > 3) {
        delete access.pending[code]
        saveAccess(access)
        return "Too many attempts. Please try again later."
      }
      saveAccess(access)
      return `Your pairing code is: ${code}\nAsk the admin to run: opencode telegram pair ${code}`
    }
    delete access.pending[code]
  }

  // Clean expired
  for (const [code, entry] of Object.entries(access.pending)) {
    if (entry.expiresAt < Date.now()) delete access.pending[code]
  }

  // Max 3 pending
  if (Object.keys(access.pending).length >= 3) {
    saveAccess(access)
    return "Too many pending pairing requests. Please try again later."
  }

  const code = generatePairingCode()
  access.pending[code] = {
    senderId,
    chatId,
    createdAt: Date.now(),
    expiresAt: Date.now() + 3600_000,
    replies: 1,
  }
  saveAccess(access)
  return `Your pairing code is: ${code}\nAsk the admin to approve with: opencode telegram pair ${code}\nThis code expires in 1 hour.`
}

// ── Approved polling ────────────────────────────────────────────────────────

function pollApproved(bot: Bot) {
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
}

// ── Gate ─────────────────────────────────────────────────────────────────────

function gate(ctx: Context): "allow" | "pair" | "deny" {
  const access = loadAccess()
  const senderId = String(ctx.from?.id ?? "")
  const chatId = String(ctx.chat?.id ?? "")

  // Group message
  if (ctx.chat?.type === "group" || ctx.chat?.type === "supergroup") {
    const groupPolicy = access.groups[chatId]
    if (!groupPolicy) return "deny"

    // Check mention requirement
    if (groupPolicy.requireMention) {
      const text = ctx.message?.text ?? ctx.message?.caption ?? ""
      const patterns = access.mentionPatterns ?? []
      const mentioned = patterns.some((p) => text.includes(p))
      // Also allow replies to the bot's own messages
      const replyToBot = ctx.message?.reply_to_message?.from?.id === bot.botInfo?.id
      if (!mentioned && !replyToBot) return "deny"
    }

    // Check group allowFrom
    if (groupPolicy.allowFrom.length > 0 && !groupPolicy.allowFrom.includes(senderId)) {
      return "deny"
    }
    return "allow"
  }

  // DM
  if (access.dmPolicy === "disabled") return "deny"
  if (access.allowFrom.includes(senderId)) return "allow"
  if (access.dmPolicy === "pairing") return "pair"
  return "deny"
}

// ── Session management ──────────────────────────────────────────────────────

// msgId -> sessionId: tracks which bot reply belongs to which session
// When bot replies, we store botReplyMsgId -> sessionId
// When user replies to a bot message, we look up the session
const msgSessions = new Map<string, string>() // `${chatId}:${botMsgId}` -> sessionId

// Per-chat message queue to ensure sequential processing
const chatQueues = new Map<string, Promise<void>>()

function enqueue(chatId: string, fn: () => Promise<void>): void {
  const prev = chatQueues.get(chatId) ?? Promise.resolve()
  const next = prev.then(fn, fn) // run fn after previous completes (even if it failed)
  chatQueues.set(chatId, next)
  // Clean up reference when done
  next.then(() => {
    if (chatQueues.get(chatId) === next) chatQueues.delete(chatId)
  })
}

// ── Main ────────────────────────────────────────────────────────────────────

console.log("Starting opencode server...")
const opencodeConfig = JSON.parse(
  readFileSync(join(process.env.HOME ?? homedir(), "opencode.json"), "utf8")
)
const opencode = await createOpencode({ port: 0, config: opencodeConfig })
console.log("Opencode server ready")

const bot = new Bot(TOKEN)
let botUsername = ""

// Fetch bot info for mention detection
try {
  const me = await bot.api.getMe()
  botUsername = me.username ?? ""
  console.log(`Bot username: @${botUsername}`)
} catch (e) {
  console.error("Failed to get bot info:", e)
}

// Poll for approved pairings
pollApproved(bot)

// ── Streaming state per session ──────────────────────────────────────────────

type StreamState = {
  chatId: string
  msgId: number | undefined
  draftId: number
  reasoning: string
  text: string
  phase: "idle" | "reasoning" | "text" | "done"
  lastDraftUpdate: number
  resolve: () => void
}

const activeStreams = new Map<string, StreamState>() // sessionId -> StreamState
const DRAFT_THROTTLE_MS = 300 // min ms between draft updates

function findChatForSession(sessionId: string): string | undefined {
  for (const [key, sid] of msgSessions.entries()) {
    if (sid === sessionId) return key.split(":")[0]!
  }
  return undefined
}

// Subscribe to opencode events for streaming + tool updates
;(async () => {
  const events = await opencode.client.event.subscribe()
  for await (const event of events.stream) {
    // Debug: log events
    if (activeStreams.size > 0 && (event.type === "message.part.updated" || event.type === "message.updated")) {
      const sid = (event.properties as any).sessionID ?? (event.properties as any).part?.sessionID
      const hasStream = activeStreams.has(sid)
      console.log("SSE:", event.type, "session:", sid, "hasStream:", hasStream, "activeKeys:", [...activeStreams.keys()].join(","))
    }

    // Handle part updates — used for both streaming and phase detection
    if (event.type === "message.part.updated") {
      const part = (event.properties as any).part
      const sessionID = part?.sessionID ?? (event.properties as any).sessionID
      const stream = activeStreams.get(sessionID)

      if (stream) {
        if (part.type === "reasoning" && part.text) {
          stream.phase = "reasoning"
          stream.reasoning = part.text
        } else if (part.type === "text" && part.text) {
          stream.phase = "text"
          stream.text = part.text
        } else if (part.type === "tool" && part.state?.status === "completed") {
          const toolMsg = `🔧 ${part.tool} — ${part.state.title ?? "done"}`
          await bot.api.sendMessage(Number(stream.chatId), toolMsg).catch(() => {})
        }

        // Throttled draft update for streaming display
        const now = Date.now()
        if ((part.type === "reasoning" || part.type === "text") && now - stream.lastDraftUpdate >= DRAFT_THROTTLE_MS) {
          stream.lastDraftUpdate = now
          const display = stream.phase === "reasoning"
            ? `💭 ${stream.reasoning}`
            : stream.text
          if (display) {
            const truncated = display.length > 3900 ? "..." + display.slice(-3900) : display
            console.log("Sending draft:", stream.phase, "len:", truncated.length, "chatId:", stream.chatId)
            await bot.api.sendMessageDraft(
              Number(stream.chatId), stream.draftId, truncated
            ).catch((e: any) => console.error("Draft error:", e?.message ?? e))
          }
        }
      }

      // Tool updates for sessions without active stream
      if (!stream && part.type === "tool" && part.state?.status === "completed") {
        const chatId = findChatForSession(sessionID)
        if (chatId) {
          const toolMsg = `🔧 ${part.tool} — ${part.state.title ?? "done"}`
          await bot.api.sendMessage(Number(chatId), toolMsg).catch(() => {})
        }
      }
    }

    // Handle message completion
    if (event.type === "message.updated") {
      const msg = (event.properties as any)
      const sessionID = msg.sessionID
      const stream = activeStreams.get(sessionID)
      if (stream && msg.role === "assistant") {
        stream.phase = "done"
        stream.resolve()
      }
    }
  }
})()

// Handle messages
async function handleMessage(ctx: Context, text: string) {
  const gateResult = gate(ctx)
  const chatId = String(ctx.chat?.id ?? "")
  const senderId = String(ctx.from?.id ?? "")

  // Gate checks are immediate (no queue needed)
  if (gateResult === "deny") return
  if (gateResult === "pair") {
    const msg = handlePairing(senderId, chatId)
    await ctx.reply(msg).catch(() => {})
    return
  }

  // Queue the actual processing per chat to avoid concurrent prompts
  enqueue(chatId, () => processMessage(ctx, text, chatId, senderId))
}

async function processMessage(ctx: Context, text: string, chatId: string, senderId: string) {
  const replyTo = ctx.message?.reply_to_message
  const replyToBot = replyTo?.from?.id === bot.botInfo?.id
  const msgId = ctx.message?.message_id

  // React immediately to acknowledge receipt
  if (msgId) {
    await bot.api.setMessageReaction(chatId, msgId, [{ type: "emoji", emoji: "👀" }]).catch(() => {})
  }

  // Determine session: reply to bot = continue, otherwise = new session
  let sessionId: string | undefined
  if (replyToBot && replyTo) {
    const key = `${chatId}:${replyTo.message_id}`
    sessionId = msgSessions.get(key)
  }

  // For DMs: also try continuing the last session for this chat if not a reply
  // (DMs are usually a single conversation thread)
  if (!sessionId && ctx.chat?.type === "private") {
    // Find most recent session for this chat
    for (const [key, sid] of msgSessions.entries()) {
      if (key.startsWith(`${chatId}:`)) {
        sessionId = sid
        // Don't break — we want the latest one (Map preserves insertion order)
      }
    }
  }

  // No existing session found → create new one
  if (!sessionId) {
    const username = ctx.from?.username ?? ctx.from?.first_name ?? "unknown"
    const chatTitle =
      ctx.chat?.type === "private"
        ? `Telegram DM: ${username}`
        : `Telegram: ${(ctx.chat as any)?.title ?? chatId}`

    const result = await opencode.client.session.create({
      body: { title: chatTitle },
    })
    if (result.error) {
      console.error("Session create error:", JSON.stringify(result.error))
      await ctx.reply("Failed to create session. Please try again.").catch(() => {})
      return
    }
    sessionId = result.data.id
  }

  // Format message with metadata
  const username = ctx.from?.username ?? ctx.from?.first_name ?? "unknown"
  const ts = new Date().toISOString()
  const prompt = `<channel source="telegram" chat_id="${chatId}" message_id="${msgId}" user="${username}" user_id="${senderId}" ts="${ts}">\n${text}\n</channel>`

  // Set up streaming state
  const draftId = Math.floor(Math.random() * 2147483647) + 1
  console.log("Registering stream for session:", sessionId, "chatId:", chatId, "draftId:", draftId)
  const streamDone = new Promise<void>((resolve) => {
    activeStreams.set(sessionId!, {
      chatId,
      msgId,
      draftId,
      reasoning: "",
      text: "",
      phase: "idle",
      lastDraftUpdate: 0,
      resolve,
    })
  })

  // Send to opencode — prompt() blocks until done, but SSE events stream in parallel
  // Fire prompt in background, let SSE handler resolve streamDone
  const promptPromise = opencode.client.session.prompt({
    path: { id: sessionId! },
    body: { parts: [{ type: "text", text: prompt }] },
  }).then((result: any) => {
    if (result.error) {
      console.error("Prompt error:", JSON.stringify(result.error))
    }
    // Resolve stream if SSE hasn't already
    const s = activeStreams.get(sessionId!)
    if (s && s.phase !== "done") {
      // Extract from prompt result as fallback
      if (!result.error && result.data) {
        const parts = result.data.parts ?? []
        s.text = s.text || parts.filter((p: any) => p.type === "text").map((p: any) => p.text).join("\n")
        s.reasoning = s.reasoning || parts.filter((p: any) => p.type === "reasoning").map((p: any) => p.text).join("\n")
      }
      s.phase = "done"
      s.resolve()
    }
  }).catch((err: any) => {
    console.error("Prompt exception:", err)
    const s = activeStreams.get(sessionId!)
    if (s) { s.phase = "done"; s.resolve() }
  })

  // Wait for streaming to complete (resolved by SSE or prompt completion)
  const timeout = setTimeout(() => {
    const stream = activeStreams.get(sessionId!)
    if (stream) stream.resolve()
  }, 300_000) // 5 min timeout

  await streamDone
  clearTimeout(timeout)

  const stream = activeStreams.get(sessionId!)
  activeStreams.delete(sessionId!)

  if (!stream) return

  // prompt() result already populated stream.text/reasoning as fallback

  // Build final message with expandable blockquote for thinking
  let finalText = ""
  if (stream.reasoning) {
    finalText += `<blockquote expandable>💭 ${escapeHtml(stream.reasoning)}</blockquote>\n\n`
  }
  finalText += escapeHtml(stream.text || "(no response)")

  // Clear draft and send final message
  const chunks = splitMessage(finalText, 4096)
  for (const chunk of chunks) {
    const sent = await bot.api.sendMessage(chatId, chunk, {
      reply_to_message_id: msgId,
      parse_mode: "HTML",
    }).catch(() => undefined)
    if (sent) {
      msgSessions.set(`${chatId}:${sent.message_id}`, sessionId!)
    }
  }

  // Clear the "processing" reaction
  if (msgId) {
    await bot.api.setMessageReaction(chatId, msgId, []).catch(() => {})
  }
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
}

function splitMessage(text: string, limit: number): string[] {
  if (text.length <= limit) return [text]
  const chunks: string[] = []
  let remaining = text
  while (remaining.length > 0) {
    if (remaining.length <= limit) {
      chunks.push(remaining)
      break
    }
    // Try to split at newline
    let splitAt = remaining.lastIndexOf("\n", limit)
    if (splitAt < limit / 2) splitAt = limit
    chunks.push(remaining.slice(0, splitAt))
    remaining = remaining.slice(splitAt).trimStart()
  }
  return chunks
}

// Register handlers
bot.on("message:text", (ctx) => handleMessage(ctx, ctx.message.text))
bot.on("message:photo", (ctx) => handleMessage(ctx, ctx.message.caption ?? "(photo)"))
bot.on("message:document", (ctx) => handleMessage(ctx, ctx.message.caption ?? `(document: ${ctx.message.document.file_name ?? "file"})`))
bot.on("message:voice", (ctx) => handleMessage(ctx, ctx.message.caption ?? "(voice message)"))
bot.on("message:video", (ctx) => handleMessage(ctx, ctx.message.caption ?? "(video)"))
bot.on("message:sticker", (ctx) => {
  const emoji = ctx.message.sticker.emoji ? ` ${ctx.message.sticker.emoji}` : ""
  return handleMessage(ctx, `(sticker${emoji})`)
})

// Start bot
bot.catch((err) => {
  console.error("Bot error:", err)
})

await bot.start({
  onStart: () => console.log(`Telegram bot @${botUsername} is running!`),
})
