import type { Bot, Context } from "grammy"
import { loadAccess, gate, stripMention, handlePairing } from "./access.js"
import {
  enqueue, trackSession, trackModelOverride,
  getSessionForReply, getLatestSessionForChat, getModelOverride,
} from "./session.js"
import { activeStreams, type StreamState } from "./stream.js"
import { toMarkdownV2, thinkingToMd2, splitMessage } from "./markdown.js"
import { getModelList, buildModelKeyboard } from "./models.js"
import { downloadTelegramFile, type AttachedFile } from "./download.js"

// ── Input sanitization ─────────────────────────────────────────────────────

function sanitizeForXml(text: string): string {
  // Escape sequences that could break out of the <channel> XML wrapper
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
}

// ── Exported setup ─────────────────────────────────────────────────────────

export function registerHandlers(bot: Bot, opencode: any) {
  // Lazy — botInfo is only available after bot.init() / bot.start()
  const getBotInfoId = () => bot.botInfo?.id

  // ── handleMessage ──────────────────────────────────────────────────────

  async function handleMessage(ctx: Context, text: string, files: AttachedFile[] = []) {
    const chatId = String(ctx.chat?.id ?? "")
    const senderId = String(ctx.from?.id ?? "")

    // Handle bot commands before gate
    const cmd = stripMention(text).trimStart().replace(/@\S+/, "").trimStart()

    if (cmd.match(/^\/list_models(\s|$)/)) {
      const models = await getModelList(opencode)
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
      const models = await getModelList(opencode)
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
    const gateResult = gate(ctx, getBotInfoId())
    if (gateResult === "deny") return
    if (gateResult === "pair") {
      const msg = handlePairing(senderId, chatId)
      await ctx.reply(msg).catch(() => {})
      return
    }

    enqueue(chatId, () => processMessage(ctx, text, chatId, senderId, files))
  }

  // ── processMessage ─────────────────────────────────────────────────────

  async function processMessage(ctx: Context, text: string, chatId: string, senderId: string, files: AttachedFile[] = []) {
    const replyTo = ctx.message?.reply_to_message
    const replyToBot = replyTo?.from?.id === getBotInfoId()
    const msgId = ctx.message?.message_id

    let cleanText = stripMention(text)

    // Parse /model command
    let modelOverride: string | undefined
    const modelMatch = cleanText.match(/^\/model[= ](\S+)\s*(.*)$/s)
    if (modelMatch) {
      modelOverride = modelMatch[1]!
      cleanText = modelMatch[2]!.trim()

      if (!cleanText && files.length === 0) {
        const sent = await ctx.reply(`Model set to ${modelOverride}. Reply to this message to start a new session.`).catch(() => undefined)
        if (sent) trackModelOverride(chatId, sent.message_id, modelOverride)
        return
      }
    }

    // Check if replying to a model-set confirmation
    if (!modelOverride && replyToBot && replyTo) {
      modelOverride = getModelOverride(chatId, replyTo.message_id)
    }

    // Detect @mention → force new session in groups
    const originalText = text
    const isGroup = ctx.chat?.type === "group" || ctx.chat?.type === "supergroup"
    const access = loadAccess()
    const patterns = access.mentionPatterns ?? []
    const startsWithMention = patterns.some((p) => originalText.trimStart().startsWith(p))
    const forceNewSession = isGroup && startsWithMention

    text = cleanText || text

    // Acknowledge receipt
    if (msgId) {
      await bot.api.setMessageReaction(chatId, msgId, [{ type: "emoji", emoji: "👀" }]).catch(() => {})
    }

    // Resolve session
    let sessionId: string | undefined

    if (!forceNewSession) {
      if (replyToBot && replyTo) {
        sessionId = getSessionForReply(chatId, replyTo.message_id)
      }
      if (!sessionId) {
        sessionId = getLatestSessionForChat(chatId)
      }
    }

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

    // Format message with sanitized user content
    const username = ctx.from?.username ?? ctx.from?.first_name ?? "unknown"
    const ts = new Date().toISOString()
    const safeText = sanitizeForXml(text)
    const safeUsername = sanitizeForXml(username)
    const prompt = `<channel source="telegram" chat_id="${chatId}" message_id="${msgId}" user="${safeUsername}" user_id="${senderId}" ts="${ts}">\n${safeText}\n</channel>`

    // Set up streaming
    const threadId = ctx.message?.message_thread_id
    const isDM = ctx.chat?.type === "private"
    const draftId = Math.floor(Math.random() * 2147483647) + 1

    let placeholderMsgId: number | undefined
    if (!isDM) {
      const opts: Record<string, unknown> = { reply_to_message_id: msgId }
      if (threadId) opts.message_thread_id = threadId
      const placeholder = await bot.api.sendMessage(chatId, "⏳", opts).catch(() => undefined)
      placeholderMsgId = placeholder?.message_id
    }

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

    // Build prompt parts
    const promptParts: any[] = [{ type: "text", text: prompt }]
    for (const file of files) {
      promptParts.push({ type: "file", mime: file.mime, url: file.dataUrl, filename: file.filename })
    }

    let promptOpts: any = { sessionID: sessionId!, parts: promptParts }
    if (modelOverride) {
      const slash = modelOverride.indexOf("/")
      if (slash > 0) {
        promptOpts.model = { providerID: modelOverride.slice(0, slash), modelID: modelOverride.slice(slash + 1) }
      }
    }

    // Fire prompt — SSE events drive streaming
    opencode.client.session.prompt(promptOpts).then((result: any) => {
      if (result.error) {
        console.error("Prompt error:", JSON.stringify(result.error))
        const s = activeStreams.get(sessionId!)
        if (s && s.phase !== "done") { s.phase = "done"; s.resolve() }
      }
    }).catch((err: any) => {
      console.error("Prompt exception:", err)
      const s = activeStreams.get(sessionId!)
      if (s && s.phase !== "done") { s.phase = "done"; s.resolve() }
    })

    // Wait for completion
    const timeout = setTimeout(() => {
      const stream = activeStreams.get(sessionId!)
      if (stream) stream.resolve()
    }, 300_000)

    await streamDone
    clearTimeout(timeout)

    const stream = activeStreams.get(sessionId!)
    activeStreams.delete(sessionId!)

    if (!stream) return

    // Clean up streaming messages
    if (stream.streamMsgId) {
      await bot.api.deleteMessage(chatId, stream.streamMsgId).catch(() => {})
    }
    if (stream.toolMsgId) {
      await bot.api.deleteMessage(chatId, stream.toolMsgId).catch(() => {})
    }

    // Build final message
    let finalText = ""
    if (stream.reasoning) {
      finalText += thinkingToMd2(stream.reasoning) + "\n\n"
    }
    finalText += toMarkdownV2(stream.text || "(no response)")

    // Send final message
    const sendOpts: Record<string, unknown> = { reply_to_message_id: msgId, parse_mode: "MarkdownV2" }
    if (stream.threadId) sendOpts.message_thread_id = stream.threadId
    const chunks = splitMessage(finalText, 4096)
    for (const chunk of chunks) {
      const sent = await bot.api.sendMessage(chatId, chunk, sendOpts).catch(() => undefined)
      if (sent) trackSession(chatId, sent.message_id, sessionId!)
    }

    // Clear reaction
    if (msgId) {
      await bot.api.setMessageReaction(chatId, msgId, []).catch(() => {})
    }
  }

  // ── Register bot handlers ──────────────────────────────────────────────

  bot.on("message:text", (ctx) => handleMessage(ctx, ctx.message.text))

  bot.on("message:photo", async (ctx) => {
    const photos = ctx.message.photo
    const largest = photos[photos.length - 1]!
    const file = await downloadTelegramFile(bot, largest.file_id, "image/jpeg", "photo.jpg")
    const files = file ? [file] : []
    return handleMessage(ctx, ctx.message.caption || "(photo)", files)
  })

  bot.on("message:document", async (ctx) => {
    const doc = ctx.message.document
    const mime = doc.mime_type ?? "application/octet-stream"
    const filename = doc.file_name ?? "file"
    const files: AttachedFile[] = []
    if (mime.startsWith("image/")) {
      const file = await downloadTelegramFile(bot, doc.file_id, mime, filename)
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

  // Model picker callbacks
  bot.on("callback_query:data", async (ctx) => {
    const data = ctx.callbackQuery.data
    if (data.startsWith("model:")) {
      const model = data.slice("model:".length)
      const chatId = String(ctx.chat?.id ?? "")
      await ctx.editMessageText(`Model set to ${model}. Reply to this message to start a new session.`).catch(() => {})
      const msgId = ctx.callbackQuery.message?.message_id
      if (msgId) trackModelOverride(chatId, msgId, model)
      await ctx.answerCallbackQuery({ text: `Selected: ${model}` }).catch(() => {})
    } else if (data.startsWith("modelpage:")) {
      const page = Number(data.slice("modelpage:".length))
      const models = await getModelList(opencode)
      await ctx.editMessageReplyMarkup({ reply_markup: buildModelKeyboard(models, page) }).catch(() => {})
      await ctx.answerCallbackQuery().catch(() => {})
    } else if (data === "noop") {
      await ctx.answerCallbackQuery().catch(() => {})
    }
  })
}
