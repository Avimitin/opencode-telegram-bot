import type { Bot } from "grammy"
import { msgSessions } from "./session.js"

// ── Types ──────────────────────────────────────────────────────────────────

export type StreamState = {
  chatId: string
  threadId: number | undefined
  isDM: boolean
  msgId: number | undefined
  streamMsgId: number | undefined
  draftId: number
  toolMsgId: number | undefined
  toolLines: string[]
  reasoning: string
  text: string
  phase: "idle" | "reasoning" | "text" | "done"
  lastStreamUpdate: number
  resolve: () => void
}

// ── State ──────────────────────────────────────────────────────────────────

export const activeStreams = new Map<string, StreamState>()
const EDIT_THROTTLE_MS = 1500
const DRAFT_THROTTLE_MS = 300

function findChatForSession(sessionId: string): string | undefined {
  for (const [key, sid] of msgSessions.entries()) {
    if (sid === sessionId) return key.split(":")[0]!
  }
  return undefined
}

// ── SSE subscriber ─────────────────────────────────────────────────────────

interface MessagePart {
  type: string
  text?: string
  sessionID?: string
  tool?: string
  state?: { status: string; title?: string }
}

interface SSEEvent {
  type: string
  properties: { sessionID?: string; part?: MessagePart }
}

export async function startSSESubscriber(opencode: any, bot: Bot) {
  const events = await opencode.client.event.subscribe()
  console.log("SSE subscriber connected")

  for await (const event of events.stream as AsyncIterable<SSEEvent>) {
    if (event.type === "message.part.updated") {
      const props = event.properties
      const part = props.part
      if (!part) continue
      const sessionID: string = props.sessionID ?? part.sessionID ?? ""
      const stream = activeStreams.get(sessionID)

      if (stream) {
        if (part.type === "reasoning" && part.text) {
          stream.phase = "reasoning"
          stream.reasoning = part.text
        } else if (part.type === "text" && part.text) {
          stream.phase = "text"
          stream.text = part.text
        } else if (part.type === "tool" && part.state?.status === "completed") {
          stream.toolLines.push(`🔧 ${part.tool} — ${part.state.title ?? "done"}`)
          const toolText = stream.toolLines.join("\n")
          if (stream.toolMsgId) {
            await bot.api.editMessageText(
              Number(stream.chatId), stream.toolMsgId, toolText
            ).catch(() => {})
          } else {
            const opts: Record<string, unknown> = {}
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
              await (bot.api as any).sendMessageDraft(
                Number(stream.chatId), stream.draftId, truncated
              ).catch(() => {})
            } else if (stream.streamMsgId) {
              await bot.api.editMessageText(
                Number(stream.chatId), stream.streamMsgId, truncated
              ).catch(() => {})
            }
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

    // Handle session idle — prompt fully done
    if (event.type === "session.idle") {
      const props = event.properties
      const sessionID: string = props.sessionID ?? ""
      const stream = activeStreams.get(sessionID)
      if (stream && stream.phase !== "done") {
        stream.phase = "done"
        stream.resolve()
      }
    }
  }
}
