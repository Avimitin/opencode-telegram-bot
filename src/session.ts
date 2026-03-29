// ── Bounded Map ────────────────────────────────────────────────────────────

const MAX_MAP_SIZE = 5000

function boundedSet<K, V>(map: Map<K, V>, key: K, value: V) {
  if (map.size >= MAX_MAP_SIZE) {
    // Evict oldest entry (first inserted)
    const firstKey = map.keys().next().value
    if (firstKey !== undefined) map.delete(firstKey)
  }
  map.set(key, value)
}

// ── Session tracking ───────────────────────────────────────────────────────

// `${chatId}:${botMsgId}` -> sessionId
export const msgSessions = new Map<string, string>()

// Per-chat model override: `${chatId}:${botMsgId}` -> "providerID/modelID"
export const msgModelOverride = new Map<string, string>()

export function trackSession(chatId: string, botMsgId: number, sessionId: string) {
  boundedSet(msgSessions, `${chatId}:${botMsgId}`, sessionId)
}

export function trackModelOverride(chatId: string, botMsgId: number, model: string) {
  boundedSet(msgModelOverride, `${chatId}:${botMsgId}`, model)
}

export function getSessionForReply(chatId: string, replyMsgId: number): string | undefined {
  return msgSessions.get(`${chatId}:${replyMsgId}`)
}

export function getLatestSessionForChat(chatId: string): string | undefined {
  let found: string | undefined
  for (const [key, sid] of msgSessions.entries()) {
    if (key.startsWith(`${chatId}:`)) found = sid
  }
  return found
}

export function getModelOverride(chatId: string, msgId: number): string | undefined {
  const key = `${chatId}:${msgId}`
  const model = msgModelOverride.get(key)
  if (model) msgModelOverride.delete(key)
  return model
}

// ── Per-chat message queue ─────────────────────────────────────────────────

const chatQueues = new Map<string, Promise<void>>()

export function enqueue(chatId: string, fn: () => Promise<void>): void {
  const prev = chatQueues.get(chatId) ?? Promise.resolve()
  const next = prev.then(fn, fn)
  chatQueues.set(chatId, next)
  next.then(() => {
    if (chatQueues.get(chatId) === next) chatQueues.delete(chatId)
  })
}
