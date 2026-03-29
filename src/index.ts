import { Bot, type Context } from "grammy"
import { createOpencode } from "@opencode-ai/sdk/v2"
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

    // Check mention requirement: @botname must be at start of message, or reply to bot
    if (groupPolicy.requireMention) {
      const text = ctx.message?.text ?? ctx.message?.caption ?? ""
      const patterns = access.mentionPatterns ?? []
      const startsWithMention = patterns.some((p) => text.trimStart().startsWith(p))
      const replyToBot = ctx.message?.reply_to_message?.from?.id === bot.botInfo?.id
      if (!startsWithMention && !replyToBot) return "deny"
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

// Per-chat model override for next new session
// Model override: stored per bot message (for reply-to-confirm flow) and per chat (fallback)
const msgModelOverride = new Map<string, string>() // `${chatId}:${botMsgId}` -> "providerID/modelID"

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
const defaultModel: string = opencodeConfig.model ?? ""
const opencode = await createOpencode({ port: 0, config: opencodeConfig })
console.log("Opencode server ready, default model:", defaultModel)

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

// Register bot commands for autocomplete
bot.api.setMyCommands([
  { command: "list_models", description: "List available models" },
  { command: "model", description: "Set model: /model provider/model" },
]).catch((e) => console.error("Failed to set commands:", e))

// ── Streaming state per session ──────────────────────────────────────────────

type StreamState = {
  chatId: string
  threadId: number | undefined
  isDM: boolean
  msgId: number | undefined
  streamMsgId: number | undefined  // placeholder message we edit for streaming (groups)
  draftId: number                  // draft ID for sendMessageDraft (DMs)
  toolMsgId: number | undefined    // single message for all tool calls, edited to append
  toolLines: string[]              // accumulated tool call lines
  reasoning: string
  text: string
  phase: "idle" | "reasoning" | "text" | "done"
  lastStreamUpdate: number
  resolve: () => void
}

const activeStreams = new Map<string, StreamState>() // sessionId -> StreamState
const EDIT_THROTTLE_MS = 1500 // min ms between edits (Telegram rate limit ~20/min per chat)
const DRAFT_THROTTLE_MS = 300  // min ms between draft updates (DMs, no rate limit)

function findChatForSession(sessionId: string): string | undefined {
  for (const [key, sid] of msgSessions.entries()) {
    if (sid === sessionId) return key.split(":")[0]!
  }
  return undefined
}

// Subscribe to opencode events for streaming + tool updates
;(async () => {
  const events = await opencode.client.event.subscribe()
  console.log("SSE subscriber connected")
  for await (const event of events.stream) {
    if (event.type === "message.part.updated") {
      const props = event.properties as any
      const part = props.part
      const sessionID: string = props.sessionID ?? part?.sessionID
      const stream = activeStreams.get(sessionID)

      if (stream) {
        if (part.type === "reasoning" && part.text) {
          stream.phase = "reasoning"
          stream.reasoning = part.text
        } else if (part.type === "text" && part.text) {
          stream.phase = "text"
          stream.text = part.text
        } else if (part.type === "tool" && part.state.status === "completed") {
          stream.toolLines.push(`🔧 ${part.tool} — ${(part.state as any).title ?? "done"}`)
          const toolText = stream.toolLines.join("\n")
          if (stream.toolMsgId) {
            await bot.api.editMessageText(
              Number(stream.chatId), stream.toolMsgId, toolText
            ).catch(() => {})
          } else {
            const opts: any = {}
            if (stream.threadId) opts.message_thread_id = stream.threadId
            const sent = await bot.api.sendMessage(Number(stream.chatId), toolText, opts).catch(() => undefined)
            if (sent) stream.toolMsgId = sent.message_id
          }
        }

        // Throttled streaming display
        const now = Date.now()
        const throttle = stream.isDM ? DRAFT_THROTTLE_MS : EDIT_THROTTLE_MS
        if ((part.type === "reasoning" || part.type === "text") && now - stream.lastStreamUpdate >= throttle) {
          stream.lastStreamUpdate = now
          const display = stream.phase === "reasoning"
            ? `💭 ${stream.reasoning}`
            : stream.text
          if (display) {
            const truncated = display.length > 3900 ? "..." + display.slice(-3900) : display
            if (stream.isDM) {
              // DM: use sendMessageDraft for native streaming look
              await bot.api.sendMessageDraft(
                Number(stream.chatId), stream.draftId, truncated
              ).catch(() => {})
            } else if (stream.streamMsgId) {
              // Group: edit placeholder message
              await bot.api.editMessageText(
                Number(stream.chatId), stream.streamMsgId, truncated
              ).catch(() => {})
            }
          }
        }
      }

      // Tool updates for sessions without active stream
      if (!stream && part.type === "tool" && part.state.status === "completed") {
        const chatId = findChatForSession(sessionID)
        if (chatId) {
          const toolMsg = `🔧 ${part.tool} — ${(part.state as any).title ?? "done"}`
          await bot.api.sendMessage(Number(chatId), toolMsg).catch(() => {})
        }
      }
    }

    // Handle session idle — means the prompt is fully done
    if (event.type === "session.idle") {
      const props = event.properties as any
      const sessionID: string = props.sessionID
      const stream = activeStreams.get(sessionID)
      if (stream && stream.phase !== "done") {
        stream.phase = "done"
        stream.resolve()
      }
    }
  }
})()

// ── File download helper ─────────────────────────────────────────────────────

type AttachedFile = { mime: string; filename: string; dataUrl: string }

async function downloadTelegramFile(fileId: string, mime: string, filename: string): Promise<AttachedFile | undefined> {
  try {
    const file = await bot.api.getFile(fileId)
    if (!file.file_path) return undefined
    const url = `https://api.telegram.org/file/bot${TOKEN}/${file.file_path}`
    const res = await fetch(url)
    if (!res.ok) return undefined
    const buf = Buffer.from(await res.arrayBuffer())
    const dataUrl = `data:${mime};base64,${buf.toString("base64")}`
    return { mime, filename, dataUrl }
  } catch (e) {
    console.error("File download error:", e)
    return undefined
  }
}

// ── Model picker helpers ─────────────────────────────────────────────────────

type ModelEntry = { fullId: string; label: string }
let cachedModels: ModelEntry[] | undefined

async function getModelList(): Promise<ModelEntry[]> {
  if (cachedModels) return cachedModels
  const result = await opencode.client.provider.list()
  if (result.error || !result.data) return []
  const models: ModelEntry[] = []
  for (const provider of result.data.all) {
    // Only include providers whose env vars are configured
    const envConfigured = provider.env.length === 0 || provider.env.some((e: string) => !!process.env[e])
    if (!envConfigured) continue
    for (const model of Object.values(provider.models)) {
      const fullId = `${provider.id}/${model.id}`
      const tags: string[] = []
      if (model.reasoning) tags.push("🧠")
      const modalities = (model as any).modalities?.input ?? []
      if (modalities.includes("image")) tags.push("🖼")
      if (model.attachment) tags.push("📎")
      const tagStr = tags.length ? ` ${tags.join("")}` : ""
      const label = `${fullId}${tagStr}`
      models.push({ fullId, label })
    }
  }
  cachedModels = models
  // Refresh cache every 5 min
  setTimeout(() => { cachedModels = undefined }, 300_000)
  return models
}

const MODELS_PER_PAGE = 6 // 6 rows of models per page

function buildModelKeyboard(models: ModelEntry[], page: number) {
  const totalPages = Math.ceil(models.length / MODELS_PER_PAGE)
  const start = page * MODELS_PER_PAGE
  const pageModels = models.slice(start, start + MODELS_PER_PAGE)
  const rows: { text: string; callback_data: string }[][] = []

  // Top nav row
  if (totalPages > 1) {
    if (page > 0) {
      rows.push([{ text: `⬅️ Page ${page}`, callback_data: `modelpage:${page - 1}` }])
    }
  }

  // One model per row
  for (const m of pageModels) {
    rows.push([{ text: m.label, callback_data: `model:${m.fullId}` }])
  }

  // Bottom nav row
  if (totalPages > 1) {
    if (page < totalPages - 1) {
      rows.push([{ text: `Page ${page + 2} ➡️`, callback_data: `modelpage:${page + 1}` }])
    }
  }

  return { inline_keyboard: rows }
}

// ── Message handling ─────────────────────────────────────────────────────────

// Strip @botname from start of text
function stripMention(text: string): string {
  const access = loadAccess()
  const patterns = access.mentionPatterns ?? []
  for (const p of patterns) {
    if (text.trimStart().startsWith(p)) {
      return text.trimStart().slice(p.length).trimStart()
    }
  }
  return text
}

// Handle messages
async function handleMessage(ctx: Context, text: string, files: AttachedFile[] = []) {
  const chatId = String(ctx.chat?.id ?? "")
  const senderId = String(ctx.from?.id ?? "")

  // Handle bot commands before gate (commands don't need allowlist)
  // Note: in groups, Telegram may append @botname to commands like /list_models@botname
  const cmd = stripMention(text).trimStart().replace(/@\S+/, "").trimStart()

  if (cmd.match(/^\/list_models(\s|$)/)) {
    const models = await getModelList()
    if (models.length === 0) {
      await ctx.reply("Failed to fetch model list.").catch(() => {})
      return
    }
    const lines = ["Available models:\n"]
    for (const m of models) lines.push(`  ${m.fullId}`)
    await ctx.reply(lines.join("\n")).catch(() => {})
    return
  }

  if (cmd.match(/^\/model\s*$/)) {
    const models = await getModelList()
    if (models.length === 0) {
      await ctx.reply("Failed to fetch model list.").catch(() => {})
      return
    }
    await ctx.reply("Select a model:", {
      reply_markup: buildModelKeyboard(models, 0),
    }).catch(() => {})
    return
  }

  // Gate check
  const gateResult = gate(ctx)
  if (gateResult === "deny") return
  if (gateResult === "pair") {
    const msg = handlePairing(senderId, chatId)
    await ctx.reply(msg).catch(() => {})
    return
  }

  // Queue the actual processing per chat to avoid concurrent prompts
  enqueue(chatId, () => processMessage(ctx, text, chatId, senderId, files))
}

async function processMessage(ctx: Context, text: string, chatId: string, senderId: string, files: AttachedFile[] = []) {
  const replyTo = ctx.message?.reply_to_message
  const replyToBot = replyTo?.from?.id === bot.botInfo?.id
  const msgId = ctx.message?.message_id

  // Parse /model command: /model=xxx or /model xxx
  let cleanText = stripMention(text)

  let modelOverride: string | undefined
  const modelMatch = cleanText.match(/^\/model[= ](\S+)\s*(.*)$/s)
  if (modelMatch) {
    modelOverride = modelMatch[1]!
    cleanText = modelMatch[2]!.trim()

    // Model-only message (no prompt): confirm and store on the confirmation message
    if (!cleanText && files.length === 0) {
      const sent = await ctx.reply(`Model set to ${modelOverride}. Reply to this message to start a new session.`).catch(() => undefined)
      if (sent) {
        msgModelOverride.set(`${chatId}:${sent.message_id}`, modelOverride)
      }
      return
    }
  }

  // Check if replying to a model-set confirmation message
  if (!modelOverride && replyToBot && replyTo) {
    const replyKey = `${chatId}:${replyTo.message_id}`
    if (msgModelOverride.has(replyKey)) {
      modelOverride = msgModelOverride.get(replyKey)
      msgModelOverride.delete(replyKey)
    }
  }

  // Check if original text starts with @botname (before stripping)
  const originalText = text
  const isGroup = ctx.chat?.type === "group" || ctx.chat?.type === "supergroup"
  const access = loadAccess()
  const patterns = access.mentionPatterns ?? []
  const startsWithMention = patterns.some((p) => originalText.trimStart().startsWith(p))
  const forceNewSession = isGroup && startsWithMention

  // Use cleanText (with @botname and /model stripped) for the prompt
  text = cleanText || text

  // React immediately to acknowledge receipt
  if (msgId) {
    await bot.api.setMessageReaction(chatId, msgId, [{ type: "emoji", emoji: "👀" }]).catch(() => {})
  }

  // Determine session strategy:
  // - Reply to bot message → continue that session
  // - DM (not reply) → continue latest session for this chat
  // - Group: @botname at start of message → new session
  // - Group: reply to bot or mention elsewhere → continue latest session
  let sessionId: string | undefined

  if (!forceNewSession) {
    // Try to continue existing session
    if (replyToBot && replyTo) {
      const key = `${chatId}:${replyTo.message_id}`
      sessionId = msgSessions.get(key)
    }
    // Fall back to latest session for this chat
    if (!sessionId) {
      for (const [key, sid] of msgSessions.entries()) {
        if (key.startsWith(`${chatId}:`)) {
          sessionId = sid
        }
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

    const result = await opencode.client.session.create({ title: chatTitle })
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

  // Set up streaming: DMs use sendMessageDraft, groups use editMessageText on placeholder
  const threadId = ctx.message?.message_thread_id
  const isDM = ctx.chat?.type === "private"
  const draftId = Math.floor(Math.random() * 2147483647) + 1

  let placeholderMsgId: number | undefined
  if (!isDM) {
    const opts: any = { reply_to_message_id: msgId }
    if (threadId) opts.message_thread_id = threadId
    const placeholder = await bot.api.sendMessage(chatId, "⏳", opts).catch(() => undefined)
    placeholderMsgId = placeholder?.message_id
  }

  // Set up streaming state
  const streamDone = new Promise<void>((resolve) => {
    activeStreams.set(sessionId!, {
      chatId,
      threadId,
      isDM: !!isDM,
      msgId,
      streamMsgId: placeholderMsgId,
      draftId,
      toolMsgId: undefined,
      toolLines: [],
      reasoning: "",
      text: "",
      phase: "idle",
      lastStreamUpdate: 0,
      resolve,
    })
  })

  // Build prompt parts: text + any attached files
  const promptParts: any[] = [{ type: "text", text: prompt }]
  for (const file of files) {
    promptParts.push({ type: "file", mime: file.mime, url: file.dataUrl, filename: file.filename })
  }

  // Parse model override into providerID/modelID
  let promptOpts: any = { sessionID: sessionId!, parts: promptParts }
  if (modelOverride) {
    const slash = modelOverride.indexOf("/")
    if (slash > 0) {
      promptOpts.model = { providerID: modelOverride.slice(0, slash), modelID: modelOverride.slice(slash + 1) }
    }
  }

  // Fire prompt in background — SSE events drive streaming
  opencode.client.session.prompt(promptOpts).then((result) => {
    if (result.error) {
      console.error("Prompt error:", JSON.stringify(result.error))
      const s = activeStreams.get(sessionId!)
      if (s && s.phase !== "done") { s.phase = "done"; s.resolve() }
    }
    // Don't resolve on success — let SSE message.updated with content handle it
  }).catch((err: any) => {
    console.error("Prompt exception:", err)
    const s = activeStreams.get(sessionId!)
    if (s && s.phase !== "done") { s.phase = "done"; s.resolve() }
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

  // Delete the streaming placeholder and tool message
  if (stream.streamMsgId) {
    await bot.api.deleteMessage(chatId, stream.streamMsgId).catch(() => {})
  }
  if (stream.toolMsgId) {
    await bot.api.deleteMessage(chatId, stream.toolMsgId).catch(() => {})
  }

  // Build final message in MarkdownV2
  let finalText = ""
  if (stream.reasoning) {
    finalText += thinkingToMd2(stream.reasoning) + "\n\n"
  }
  finalText += toMarkdownV2(stream.text || "(no response)")

  // Send final message
  const sendOpts: any = { reply_to_message_id: msgId, parse_mode: "MarkdownV2" }
  if (stream.threadId) sendOpts.message_thread_id = stream.threadId
  const chunks = splitMessage(finalText, 4096)
  for (const chunk of chunks) {
    const sent = await bot.api.sendMessage(chatId, chunk, sendOpts).catch(() => undefined)
    if (sent) {
      msgSessions.set(`${chatId}:${sent.message_id}`, sessionId!)
    }
  }

  // Clear the "processing" reaction
  if (msgId) {
    await bot.api.setMessageReaction(chatId, msgId, []).catch(() => {})
  }
}

// ── MarkdownV2 formatting ───────────────────────────────────────────────────

const MD2_SPECIAL = /([_*\[\]()~`>#+\-=|{}.!\\])/g

function escapeMd2(text: string): string {
  return text.replace(MD2_SPECIAL, "\\$1")
}

// Convert LLM markdown output to Telegram MarkdownV2
function toMarkdownV2(text: string): string {
  const segments: string[] = []
  let pos = 0

  // Regex to match code blocks and inline code
  const codeBlockRe = /```(\w*)\n([\s\S]*?)```/g
  const inlineCodeRe = /`([^`\n]+)`/g

  // First pass: extract code blocks, convert remaining text
  // We process the text by splitting around code blocks
  const parts: { type: "text" | "codeblock" | "inline"; content: string; lang?: string }[] = []

  // Find all code blocks first
  const blocks: { start: number; end: number; lang: string; code: string }[] = []
  let m: RegExpExecArray | null
  while ((m = codeBlockRe.exec(text)) !== null) {
    blocks.push({ start: m.index, end: m.index + m[0].length, lang: m[1]!, code: m[2]! })
  }

  // Build parts array
  let lastEnd = 0
  for (const block of blocks) {
    if (block.start > lastEnd) {
      parts.push({ type: "text", content: text.slice(lastEnd, block.start) })
    }
    parts.push({ type: "codeblock", content: block.code, lang: block.lang })
    lastEnd = block.end
  }
  if (lastEnd < text.length) {
    parts.push({ type: "text", content: text.slice(lastEnd) })
  }

  // Process each part
  for (const part of parts) {
    if (part.type === "codeblock") {
      // Code blocks: only escape ` and \ inside
      const escaped = part.content.replace(/\\/g, "\\\\").replace(/`/g, "\\`")
      segments.push(`\`\`\`${part.lang ?? ""}\n${escaped}\`\`\``)
    } else {
      // Process text: handle inline code, bold, italic, links, then escape the rest
      let t = part.content

      // Temporarily replace inline code
      const inlineCodes: string[] = []
      t = t.replace(inlineCodeRe, (_, code) => {
        const idx = inlineCodes.length
        const escaped = code.replace(/\\/g, "\\\\").replace(/`/g, "\\`")
        inlineCodes.push("`" + escaped + "`")
        return `\x00IC${idx}\x00`
      })

      // Temporarily replace links [text](url)
      const links: string[] = []
      t = t.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_, linkText, url) => {
        const idx = links.length
        links.push(`[${escapeMd2(linkText)}](${url.replace(/[)\\]/g, "\\$&")})`)
        return `\x00LK${idx}\x00`
      })

      // Convert **bold** → *bold* (Telegram uses single *)
      t = t.replace(/\*\*(.+?)\*\*/g, (_, inner) => `\x00BS\x00${inner}\x00BE\x00`)

      // Convert _italic_ or *italic* (single) → _italic_
      // Only single * that aren't part of ** (already handled)
      t = t.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, (_, inner) => `\x00IS\x00${inner}\x00IE\x00`)

      // Escape remaining special chars
      t = escapeMd2(t)

      // Restore bold markers
      t = t.replace(/\x00BS\x00/g, "*").replace(/\x00BE\x00/g, "*")
      // Restore italic markers
      t = t.replace(/\x00IS\x00/g, "_").replace(/\x00IE\x00/g, "_")

      // Restore inline codes
      t = t.replace(/\x00IC(\d+)\x00/g, (_, idx) => inlineCodes[Number(idx)]!)
      // Restore links
      t = t.replace(/\x00LK(\d+)\x00/g, (_, idx) => links[Number(idx)]!)

      segments.push(t)
    }
  }

  return segments.join("")
}

// Format thinking as MarkdownV2 expandable blockquote (plain text, no markdown)
function thinkingToMd2(text: string): string {
  const escaped = escapeMd2(text)
  const lines = escaped.split("\n")
  if (lines.length === 0) return ""
  // First line: >💭 ..., middle lines: >..., last line ends with ||
  const result = lines.map((l, i) => {
    const prefix = i === 0 ? ">💭 " : ">"
    const suffix = i === lines.length - 1 ? "||" : ""
    return `${prefix}${l}${suffix}`
  })
  return result.join("\n")
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
bot.on("message:photo", async (ctx) => {
  // Get the largest photo (last in array)
  const photos = ctx.message.photo
  const largest = photos[photos.length - 1]!
  const file = await downloadTelegramFile(largest.file_id, "image/jpeg", "photo.jpg")
  const files = file ? [file] : []
  return handleMessage(ctx, ctx.message.caption || "(photo)", files)
})
bot.on("message:document", async (ctx) => {
  const doc = ctx.message.document
  const mime = doc.mime_type ?? "application/octet-stream"
  const filename = doc.file_name ?? "file"
  const files: AttachedFile[] = []
  // Download images sent as documents
  if (mime.startsWith("image/")) {
    const file = await downloadTelegramFile(doc.file_id, mime, filename)
    if (file) files.push(file)
  }
  return handleMessage(ctx, ctx.message.caption || `(document: ${filename})`, files)
})
bot.on("message:voice", (ctx) => handleMessage(ctx, ctx.message.caption ?? "(voice message)"))
bot.on("message:video", (ctx) => handleMessage(ctx, ctx.message.caption ?? "(video)"))
bot.on("message:sticker", (ctx) => {
  const emoji = ctx.message.sticker.emoji ? ` ${ctx.message.sticker.emoji}` : ""
  return handleMessage(ctx, `(sticker${emoji})`)
})

// Handle model picker callbacks
bot.on("callback_query:data", async (ctx) => {
  const data = ctx.callbackQuery.data
  if (data.startsWith("model:")) {
    const model = data.slice("model:".length)
    const chatId = String(ctx.chat?.id ?? "")
    await ctx.editMessageText(`Model set to ${model}. Reply to this message to start a new session.`).catch(() => {})
    const msgId = ctx.callbackQuery.message?.message_id
    if (msgId) {
      msgModelOverride.set(`${chatId}:${msgId}`, model)
    }
    await ctx.answerCallbackQuery({ text: `Selected: ${model}` }).catch(() => {})
  } else if (data.startsWith("modelpage:")) {
    const page = Number(data.slice("modelpage:".length))
    const models = await getModelList()
    await ctx.editMessageReplyMarkup({ reply_markup: buildModelKeyboard(models, page) }).catch(() => {})
    await ctx.answerCallbackQuery().catch(() => {})
  } else if (data === "noop") {
    await ctx.answerCallbackQuery().catch(() => {})
  }
})

// Start bot
bot.catch((err) => {
  console.error("Bot error:", err)
})

await bot.start({
  onStart: () => console.log(`Telegram bot @${botUsername} is running!`),
})
